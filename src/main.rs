//! 入口：系统托盘（tray-icon + winit 事件循环，主线程）+ Axum 服务（后台线程）。
//! release 构建自动隐藏控制台窗口（windows_subsystem）。

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod graph;
mod kb;
mod llm;
mod search;
mod server;

use std::path::{Path, PathBuf};
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use winit::application::ApplicationHandler;
use winit::event::StartCause;
use winit::event_loop::{ActiveEventLoop, EventLoop};

fn main() {
    let mut port: u16 = std::env::var("MD_AGENT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8756);
    let mut no_tray = std::env::var("MD_AGENT_NO_TRAY").is_ok_and(|v| v == "1");

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
            "-h" | "--help" => {
                print_help();
                return;
            }
            _ => {}
        }
    }

    let kb_root = kb::kb_root();
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
}

struct App {
    url: String,
    kb_root: PathBuf,
    open_id: MenuId,
    sync_id: MenuId,
    quit_id: MenuId,
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
        if let UserEvent::Menu(ev) = event {
            if ev.id == self.quit_id {
                std::process::exit(0);
            } else if ev.id == self.open_id {
                open_browser(&self.url);
            } else if ev.id == self.sync_id {
                match kb::sync_index(&self.kb_root) {
                    Ok(r) => eprintln!("INDEX 已重建: {} 篇", r.files),
                    Err(e) => eprintln!("INDEX 重建失败: {e}"),
                }
                match graph::sync_graph(&self.kb_root) {
                    Ok(g) => eprintln!("图谱已重建: {} 文档 / {} 链接 / {} 悬空", g.docs, g.links, g.dangling),
                    Err(e) => eprintln!("图谱重建失败: {e}"),
                }
            }
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

    let menu = Menu::new();
    let open_item = MenuItem::new("打开终端", true, None);
    let sync_item = MenuItem::new("同步索引", true, None);
    let quit_item = MenuItem::new("退出", true, None);
    let _ = menu.append(&open_item);
    let _ = menu.append(&sync_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);
    let open_id = open_item.id().clone();
    let sync_id = sync_item.id().clone();
    let quit_id = quit_item.id().clone();

    let mut app = App {
        url: url.to_string(),
        kb_root: kb_root.to_path_buf(),
        open_id,
        sync_id,
        quit_id,
        menu: Some(menu),
        tray: None,
    };

    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("托盘事件循环错误: {e}");
    }
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

/// 32x32 RGBA 图标：白边 + 深蓝底（示意标识，打包时可换成真图标）
fn gen_icon() -> tray_icon::Icon {
    let (w, h) = (32u32, 32u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let border = x < 2 || y < 2 || x >= w - 2 || y >= h - 2;
            if border {
                rgba[i..i + 4].copy_from_slice(&[240, 240, 245, 255]);
            } else {
                rgba[i..i + 4].copy_from_slice(&[56, 90, 190, 255]);
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
