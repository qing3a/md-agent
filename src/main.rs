//! 入口：系统托盘（tray-icon + winit 事件循环，主线程）+ Axum 服务（后台线程）。
//! release 构建自动隐藏控制台窗口（windows_subsystem）。

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod consolidate;
mod fetch;
mod heartbeat;
mod ingest;
mod hub;
mod graph;
mod kb;
mod llm;
mod market;
mod mcp;
mod search;
mod server;
mod page;
mod risk;
mod task;
mod activity;
mod projects;

use std::path::{Path, PathBuf};
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use winit::application::ApplicationHandler;
use winit::event::StartCause;
use winit::event_loop::{ActiveEventLoop, EventLoop};

fn main() {
    let mut port: u16 = std::env::var("MD_AGENT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8756);
    let mut no_tray = std::env::var("MD_AGENT_NO_TRAY").is_ok_and(|v| v == "1");
    let mut mcp_mode = false;

    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => {
                if let Some(v) = args.next() {
                    if let Ok(p) = v.parse() {
                        port = p;
                    }
                }
            }
            "--no-tray" => no_tray = true,
            // MCP 薄壳模式：stdio JSON-RPC server（Claude Code / Harness / Cursor 一行配置接入）
            "--mcp" => mcp_mode = true,
            "-h" | "--help" => {
                print_help();
                return;
            }
            _ => {}
        }
    }

    let kb_root = resolved_kb_root();
    let web_dir = web_dir();
    if let Err(e) = kb::ensure_layout(&kb_root) {
        eprintln!("初始化 KB 失败: {e}");
        std::process::exit(1);
    }

    let url = format!("http://127.0.0.1:{port}");

    // HTTP 服务线程
    let s_kb = kb_root.clone();
    let s_web = web_dir.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("创建 tokio 运行时失败: {e}");
                std::process::exit(1);
            }
        };
        match rt.block_on(server::serve(port, s_kb, s_web)) {
            Ok(()) => eprintln!("HTTP 服务已退出 (127.0.0.1:{port})"),
            Err(e) => {
                eprintln!("HTTP 服务失败 (127.0.0.1:{port}): {e}");
                std::process::exit(1);
            }
        }
    });

    println!("md-agent 已启动: {url}   KB: {}", kb_root.display());
    println!("检索示例: {url}/api/search?q=检索&layer=all   重建索引: POST /api/kb/sync");

    // MCP 薄壳模式：stdio JSON-RPC 主循环（阻塞；HTTP 服务线程已在上方启动）
    if mcp_mode {
        crate::mcp::run_stdio(port);
        return;
    }

    if no_tray {
        println!("[--no-tray] 开发模式，Ctrl+C 退出。");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    run_tray(&url, &kb_root);
}

fn web_dir() -> PathBuf {
    if Path::new("web").is_dir() {
        return PathBuf::from("web");
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("web")))
        .unwrap_or_else(|| PathBuf::from("web"))
}

/// KB 根目录：config.json 的 kb_root 优先（相对路径按当前工作目录解析），
/// 未配置时回落 kb::kb_root()（env MD_AGENT_KB > ./kb > exe 旁 kb）。改动需重启生效。
fn resolved_kb_root() -> PathBuf {
    let cfg = config::load();
    let p = cfg.kb_root.trim();
    if p.is_empty() {
        return kb::kb_root();
    }
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(pb)
    } else {
        pb
    }
}

fn print_help() {
    println!(
        "md-agent — 本地双层 MD 知识库 Agent\n\
         \n\
         用法: md-agent [--port <端口>] [--no-tray]\n\
         \n\
         环境变量:\n\
           MD_AGENT_KB       KB 根目录（默认: ./kb 或 exe 旁 kb）\n\
           MD_AGENT_PORT     服务端口（默认 8756）\n\
           MD_AGENT_CONFIG   config.json 路径\n\
           MD_AGENT_NO_TRAY  1 时等同 --no-tray"
    );
}

// ---------- 托盘 ----------

enum UserEvent {
    // 托盘图标事件暂不消费（后续可做单击打开终端）
    Tray,
    Menu(tray_icon::menu::MenuEvent),
    /// 30s 定时：重建动态托盘菜单（应用安装/卸载后自动反映）
    RefreshTray,
}

/// 托盘固定菜单项 id（动态子菜单「已安装应用」用 "app:<id>" 前缀区分）
struct TrayIds {
    open: MenuId,
    market: MenuId,
    sync: MenuId,
    hb: MenuId,
    key: MenuId,
    quit: MenuId,
}

struct App {
    url: String,
    kb_root: PathBuf,
    ids: TrayIds,
    menu: Option<Menu>,
    tray: Option<tray_icon::TrayIcon>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, _el: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _el: &ActiveEventLoop,
        _id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        // 事件循环真正运行后再建托盘图标（避免平台侧显示问题）
        if matches!(cause, StartCause::Init) {
            if let Some(menu) = self.menu.take() {
                self.tray = Some(build_tray(menu));
            }
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(ev) => {
                if ev.id == self.ids.quit {
                    std::process::exit(0);
                } else if ev.id == self.ids.open {
                    open_browser(&self.url);
                } else if ev.id == self.ids.market {
                    open_browser(&format!("{}?view=market", self.url));
                } else if ev.id == self.ids.sync {
                    // 手动全量同步（INDEX + 图谱）：走本地服务端点，与心跳共用 sync_lock 防并发
                    let url = self.url.clone();
                    std::thread::spawn(move || {
                        let rt = match tokio::runtime::Runtime::new() {
                            Ok(rt) => rt,
                            Err(_) => return,
                        };
                        rt.block_on(async {
                            let c = reqwest::Client::new();
                            let _ = c.post(format!("{url}/api/kb/sync")).send().await;
                            let _ = c.post(format!("{url}/api/graph/sync")).send().await;
                        });
                    });
                    eprintln!("托盘同步: 已触发 INDEX + 图谱重建");
                } else if ev.id == self.ids.hb {
                    // 勾选翻转：写 config（≤1 个心跳周期生效）；重建菜单反映新勾选态
                    let mut cfg = crate::config::load();
                    cfg.heartbeat.enabled = !cfg.heartbeat.enabled;
                    let _ = crate::config::save(&cfg);
                    eprintln!("心跳自动同步: {}", if cfg.heartbeat.enabled { "开" } else { "关" });
                    self.rebuild_tray();
                } else if ev.id == self.ids.key {
                    open_browser(&format!("{}/config.html#key", self.url));
                } else if ev.id.0.starts_with("app:") {
                    // 已安装应用子菜单 → 打开应用视图
                    let id = ev.id.0.trim_start_matches("app:");
                    open_browser(&format!("{}?view={}", self.url, id));
                } else if ev.id.0.starts_with("panel:") {
                    // 面板导航子菜单 → 浏览器打开对应 ?view= 面板
                    let id = ev.id.0.trim_start_matches("panel:");
                    open_browser(&format!("{}?view={}", self.url, id));
                }
            }
            UserEvent::RefreshTray => self.rebuild_tray(),
            UserEvent::Tray => {}
        }
    }
}

impl App {
    /// 重建动态托盘菜单（30s 定时 + 心跳切换 + 应用安装/卸载后）
    fn rebuild_tray(&mut self) {
        if let Some(tray) = &self.tray {
            let (menu, ids) = build_menu(&self.kb_root);
            let _ = tray.set_menu(Some(Box::new(menu)));
            self.ids = ids;
        }
    }
}

fn run_tray(url: &str, kb_root: &Path) {
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("创建事件循环失败: {e}");
            return;
        }
    };

    // 托盘事件转发到 winit 用户事件
    let proxy = event_loop.create_proxy();
    tray_icon::TrayIconEvent::set_event_handler(Some(move |_e| {
        let _ = proxy.send_event(UserEvent::Tray);
    }));
    let proxy = event_loop.create_proxy();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let (menu, ids) = build_menu(kb_root);
    let mut app = App {
        url: url.to_string(),
        kb_root: kb_root.to_path_buf(),
        ids,
        menu: Some(menu),
        tray: None,
    };

    // 托盘动态菜单（工作台/应用市场阶段 3）：30s 定时重建——应用安装/卸载后自动反映
    {
        let proxy = event_loop.create_proxy();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let _ = proxy.send_event(UserEvent::RefreshTray);
        });
    }

    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("托盘事件循环错误: {e}");
    }
}

/// 构建动态托盘菜单：导航组（打开终端 / 工作台）+ 已安装应用子菜单（动态）+ 面板导航子菜单
/// + 设置组（心跳同步勾选 / Key 设置）+ 退出；分隔线分组便于扫读
fn build_menu(kb_root: &Path) -> (Menu, TrayIds) {
    let menu = Menu::new();
    let open_item = MenuItem::new("打开终端", true, None);
    let market_item = MenuItem::new("工作台", true, None);
    let sync_item = MenuItem::new("同步", true, None);
    // 复选框 + 文字态双信号：对勾在部分主题下不明显，文字「开/关」保证反馈可见。
    // 注意 with_id 参数顺序：(id, text, enabled, checked, accelerator) —— enabled 恒为 true，
    // 若把 checked 传进 enabled，关态项会被原生置灰而点不动。
    let hb_on = crate::config::load().heartbeat.enabled;
    let hb_item = CheckMenuItem::with_id(
        "hb-sync",
        if hb_on { "心跳同步：开" } else { "心跳同步：关" },
        true,
        hb_on,
        None,
    );
    let key_item = MenuItem::new("Key 设置", true, None);
    let quit_item = MenuItem::new("退出", true, None);
    // 导航组
    let _ = menu.append(&open_item);
    let _ = menu.append(&market_item);
    let _ = menu.append(&sync_item);
    // 已安装应用子菜单（动态：kb/apps/*/app.json）
    let apps = crate::kb::list_apps(kb_root);
    if !apps.is_empty() {
        let _ = menu.append(&PredefinedMenuItem::separator());
        let sub = Submenu::new("已安装应用", true);
        for a in &apps {
            let it = MenuItem::with_id(format!("app:{}", a.id), format!("{} v{}", a.name, a.version), true, None);
            let _ = sub.append(&it);
        }
        let _ = menu.append(&sub);
    }
    // 面板导航子菜单（固定：待审/看板/审计/图谱 → 浏览器开 ?view= 面板）
    let panel_sub = Submenu::new("面板", true);
    for (id, name) in [("pending", "待审"), ("board", "看板"), ("audit", "审计"), ("graph", "图谱")] {
        let it = MenuItem::with_id(format!("panel:{id}"), name, true, None);
        let _ = panel_sub.append(&it);
    }
    let _ = menu.append(&panel_sub);
    // 设置组
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&hb_item);
    let _ = menu.append(&key_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);
    let ids = TrayIds {
        open: open_item.id().clone(),
        market: market_item.id().clone(),
        sync: sync_item.id().clone(),
        hb: hb_item.id().clone(),
        key: key_item.id().clone(),
        quit: quit_item.id().clone(),
    };
    (menu, ids)
}

fn build_tray(menu: Menu) -> tray_icon::TrayIcon {
    let icon = gen_icon();
    tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("md-agent 本地知识库")
        .with_icon(icon)
        .build()
        .expect("创建托盘图标失败")
}

/// 32x32 RGBA 图标：白色圆角文档 + 深蓝知识行 + 青色链接点（MD 知识库意象）
fn gen_icon() -> tray_icon::Icon {
    let (w, h) = (32u32, 32u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let blue = [56u8, 90u8, 190u8, 255u8];
    let white = [238u8, 242u8, 255u8, 255u8];
    let gray = [148u8, 158u8, 190u8, 255u8];
    let cyan = [64u8, 192u8, 190u8, 255u8];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            // 圆角文档主体（白）
            let page = x >= 7 && x <= 26 && y >= 4 && y <= 29;
            let corner = (x <= 9 && y <= 6) || (x >= 24 && y <= 6) || (x <= 9 && y >= 27) || (x >= 24 && y >= 27);
            if page && !corner {
                rgba[i..i + 4].copy_from_slice(&white);
            } else {
                rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
            if page && !corner {
                // 文档折角（右上小三角）
                if x >= 22 && y <= 9 && x + y >= 32 {
                    rgba[i..i + 4].copy_from_slice(&gray);
                }
                // 知识行（深蓝）
                if (y == 13 && x >= 10 && x <= 21) || (y == 18 && x >= 10 && x <= 23) || (y == 23 && x >= 10 && x <= 19) {
                    rgba[i..i + 4].copy_from_slice(&blue);
                }
                // 链接点（青色）
                let dx = x as i32 - 24;
                let dy = y as i32 - 13;
                if dx * dx + dy * dy <= 9 {
                    rgba[i..i + 4].copy_from_slice(&cyan);
                }
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, w, h).expect("生成图标失败")
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
