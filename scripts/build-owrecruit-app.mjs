#!/usr/bin/env node
/**
 * build-owrecruit-app.mjs — 把 xx2（ow-recruit，Vite ES-module SPA）打包成
 * md-agent 单文件 HTML app 包（kb/apps/ow-recruit/ 的输入）。
 *
 * 背景（三条沙箱硬约束）：
 *   1. 沙箱 iframe（allow-scripts，无 allow-same-origin）= opaque origin，
 *      `<script type="module" src>` 与动态 import 走 CORS 取模块 → 被拦 →
 *      必须单 chunk 内联（rollup inlineDynamicImports + format 'es'）。
 *   2. 沙箱无 IndexedDB → xx2 的 local-db.js 在 window.__OW_IN_APP__ 下切 localStorage 后端
 *      （桥层把 localStorage 代理到 /api/apps/<id>/data，声明 storage 权限即可）。
 *   3. 沙箱内非 /api/* 的 fetch 是 opaque origin 跨域请求 → 远程 API 直连被 CORS 拦 →
 *      纯本地模式（xx2 自带 local fallback，ERP/协作自动降级）。
 *
 * 启动引导（内联于 bundle 首部，模块脚本 top-level await）：
 *   - window.__OW_IN_APP__ = true
 *   - await hostApi('/api/apps/<id>/data') 预载持久化数据，写入 localStorage（桥代理防抖落盘）。
 *     模块脚本天然 defer + await 保证应用代码在数据就绪后才执行，消除
 *     「桥异步加载 vs 模块同步读」的启动竞态。
 *
 * 用法：node scripts/build-owrecruit-app.mjs [--xx2 <路径>] [--out <目录>]
 *   默认 xx2 = C:\Users\Administrator\Desktop\xx2；默认 out = <repo>/.build/ow-recruit
 *   产物 = out/index.html（自包含单文件）+ out/app.json（manifest）
 */

import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const REPO = path.resolve(__dirname, '..')

const args = process.argv.slice(2)
function argVal(flag, def) {
  const i = args.indexOf(flag)
  return i !== -1 && args[i + 1] ? args[i + 1] : def
}
const XX2 = path.resolve(argVal('--xx2', 'C:\\Users\\Administrator\\Desktop\\xx2'))
const OUT = path.resolve(argVal('--out', path.join(REPO, '.build', 'ow-recruit')))

if (!fs.existsSync(path.join(XX2, 'package.json'))) {
  console.error('xx2 目录无效（缺 package.json）: ' + XX2)
  process.exit(1)
}

const APP_ID = 'ow-recruit'
const APP_NAME = 'Ow Recruit 招聘工作台'
const APP_DESC = 'PM 招聘需求建模 + 猎头招聘执行一体化工作台（候选人/职位/面试/协作/招聘漏斗）'
const pkg = JSON.parse(fs.readFileSync(path.join(XX2, 'package.json'), 'utf8'))

const require = createRequire(path.join(XX2, 'package.json'))
const { build } = require('vite')

// 启动引导：插入内联 module script 首部（hostApi 由 md-agent BRIDGE 注入）
// 非 md-agent 环境（直接浏览器打开单文件）时 hostApi 不存在 → 跳过预载，localStorage 原生可用
const BOOT = [
  'window.__OW_IN_APP__ = true;',
  '/* md-agent 沙箱启动引导：预载持久化数据 → 写入 localStorage（桥代理防抖落盘）。',
  '   模块脚本 top-level await 保证应用代码在数据就绪后才执行，消除启动竞态。 */',
  'if (typeof window.hostApi === "function") {',
  '  await window.hostApi("/api/apps/" + (window.__appId || "") + "/data")',
  '    .then(function (__d) {',
  '      var __seed = __d && __d.data && typeof __d.data === "object" ? __d.data : null;',
  '      if (__seed) { for (var __k in __seed) { try { window.localStorage.setItem(__k, String(__seed[__k])); } catch (__e) {} } }',
  '    })',
  '    .catch(function () {});',
  '}',
].join('\n')

// 剔除沙箱内无用的 agent 适配脚本 / 测试 UI（events.js 对 #test-output 用可选链，缺失安全）
const stripPlugin = {
  name: 'ow-recruit-strip',
  transformIndexHtml(html) {
    return html
      .replace(/<script[^>]*src="[^"]*page-agent-(ux|adapter)\.js[^"]*"[^>]*>\s*<\/script>/g, '')
      .replace(/<link[^>]*rel="manifest"[^>]*>/g, '')
      .replace(/<div style="position:fixed;bottom:1rem;right:1rem[^>]*>[\s\S]*?<\/div>/, '')
      .replace(/<!-- Test runner output[\s\S]*?<\/pre>/, '')
  },
}

const buildDir = path.join(REPO, '.build')
fs.mkdirSync(buildDir, { recursive: true })
const tmpOut = fs.mkdtempSync(path.join(buildDir, 'ow-vite-'))

console.log('[ow-app] vite build:', XX2)
try {
  await build({
    root: XX2,
    configFile: false, // 不用 dev 配置（server.proxy /v1 等与本构建无关）
    logLevel: 'warn',
    base: './',
    build: {
      outDir: tmpOut,
      emptyOutDir: true,
      target: 'es2022', // 保留 top-level await
      minify: process.env.OW_NO_MINIFY ? false : 'esbuild', // 诊断用：OW_NO_MINIFY=1 出可读代码
      assetsInlineLimit: 100000000, // 资源一律 data URI 内联
      cssCodeSplit: false, // 单 CSS
      rollupOptions: {
        input: path.join(XX2, 'prototype.html'),
        output: {
          format: 'es',
          inlineDynamicImports: true, // 把 router-registries 的动态 import 内联进单 chunk
        },
      },
    },
    plugins: [stripPlugin],
  })
} catch (e) {
  console.error('[ow-app] vite build 失败:', e)
  process.exit(1)
}

// ---- 后处理：把 dist 的 JS/CSS 内联进 HTML，产出单文件 ----
// 入口是 prototype.html → vite 产物也叫 prototype.html（MPA 风格按入口命名）
const entryHtml = fs.readdirSync(tmpOut).find((f) => f.endsWith('.html'))
if (!entryHtml) {
  console.error('[ow-app] 构建产物中未找到 HTML')
  process.exit(1)
}
const distHtml = fs.readFileSync(path.join(tmpOut, entryHtml), 'utf8')

let jsInline = null
let cssInline = null
const assetsDir = path.join(tmpOut, 'assets')
if (fs.existsSync(assetsDir)) {
  for (const f of fs.readdirSync(assetsDir)) {
    const p = path.join(assetsDir, f)
    if (f.endsWith('.js') && jsInline === null) jsInline = fs.readFileSync(p, 'utf8')
    else if (f.endsWith('.css') && cssInline === null) cssInline = fs.readFileSync(p, 'utf8')
  }
}

let final = distHtml
if (jsInline !== null) {
  // 防内联 JS 中的字符串 "</script>" 截断 HTML
  const safeJs = jsInline.replace(/<\/script/gi, '<\\/script')
  // ⚠️ 必须用替换函数而非替换字符串：minified bundle 含 $& / $1 等模式，
  // 字符串替换会被 String.replace 解释（$& = 匹配到的标签），导致标签文本渗入 bundle
  final = final.replace(
    /<script type="module"[^>]*src="[^"]*"[^>]*>\s*<\/script>/,
    () => '<script type="module">\n' + BOOT + '\n' + safeJs + '\n</script>'
  )
}
if (cssInline !== null) {
  final = final.replace(
    /<link rel="stylesheet"[^>]*href="[^"]*"[^>]*>/,
    '<style>\n' + cssInline + '\n</style>'
  )
}

// 残留外部引用检查（除 data: URI 外不应再有 src=/href=/url(）
const leftovers = (final.match(/(?:src|href)=["'](?!data:|#)[^"']*["']/g) || [])
  .concat((final.match(/url\(['"]?[^)'"]+\)/g) || []).filter((u) => !/^url\((?:data:|#)/.test(u)))
if (leftovers.length) {
  console.warn('[ow-app] ⚠️ 单文件残留外部引用（需人工确认）:', leftovers.slice(0, 10))
}

fs.mkdirSync(OUT, { recursive: true })
fs.writeFileSync(path.join(OUT, 'index.html'), final)
fs.writeFileSync(
  path.join(OUT, 'app.json'),
  JSON.stringify(
    {
      id: APP_ID,
      name: APP_NAME,
      version: pkg.version || '0.1.0',
      entry: 'index.html',
      permissions: ['storage'],
      description: APP_DESC,
    },
    null,
    2
  )
)
if (process.env.OW_KEEP_TMP) {
  console.log('[ow-app] 保留 vite 原始产物（调试）: ' + tmpOut)
} else {
  fs.rmSync(tmpOut, { recursive: true, force: true })
}

console.log('[ow-app] ✅ 打包完成 →', OUT)
console.log(
  '[ow-app] index.html:',
  (final.length / 1024).toFixed(0) + 'KB',
  '| js:',
  (jsInline || '').length / 1024 > 0 ? ((jsInline || '').length / 1024).toFixed(0) + 'KB' : 'n/a',
  '| css:',
  cssInline ? (cssInline.length / 1024).toFixed(0) + 'KB' : 'n/a'
)
console.log('[ow-app] 安装：cp -r "' + OUT + '" "<repo>/kb/apps/"（或 /market import ' + OUT + ' 走人审）')
