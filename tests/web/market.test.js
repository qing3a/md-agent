#!/usr/bin/env node
/* market.html 面板逻辑测试（Node 桩环境跑真实脚本）：
 * 已安装优先：默认 Tab / 目录排序 / [已安装] 徽标
 * 运行: node --test tests/web/market.test.js */
const { test } = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');

// ---- 最小 DOM 桩 ----
function makeEl(id) {
  const el = {
    id, _text: '', _html: '', _style: {}, _cls: new Set(),
    style: { display: '' }, className: '',
    addEventListener() {}, dataset: {},
    set textContent(v) { this._text = String(v); },
    get textContent() { return this._text; },
    set innerHTML(v) { this._html = String(v); },
    get innerHTML() { return this._html; },
  };
  el.classList = { toggle(c, on) { el._cls[on ? 'add' : 'delete'](c); } };
  return el;
}
const els = {};
const getEl = (id) => (els[id] = els[id] || makeEl(id));
global.document = { getElementById: getEl, querySelectorAll: () => [], addEventListener: () => {} };
global.window = { parent: { postMessage() {} } };

const catalogApps = [
  { id: 'ow-recruit', name: '猎头招聘台', version: '1.0', hub: 'skillhub', permissions: ['storage'] },
  { id: 'zzz-new-app', name: '新应用', version: '0.1', hub: 'skillhub', permissions: [] },
  { id: 'match', name: '相亲评估工作台', version: '0.2', hub: 'skillhub', permissions: ['llm'] },
];
const installedApps = [
  { id: 'match', name: '相亲评估工作台', version: '0.2', entry: 'index.html', permissions: ['llm'], source_hub: null },
  { id: 'ow-recruit', name: '猎头招聘台', version: '1.0', entry: 'index.html', permissions: ['storage'], source_hub: null },
];
global.hostApi = (p) => Promise.resolve(
  p === '/api/hubs' ? { hubs: [] }
  : p === '/api/market/catalog' ? { apps: catalogApps }
  : p === '/api/apps' ? { apps: installedApps } : {}
);

const html = fs.readFileSync(path.join(__dirname, '../../web/views/market.html'), 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
eval(script);

test('市场视图首次加载默认落在「已安装」Tab（有已装应用时）', async () => {
  await load();       // 工作台数据（apps 就位）
  await loadMarket(); // 进入市场视图（触发已安装优先逻辑）
  assert.ok(getEl('tab-installed')._cls.has('sel'), '已安装 Tab 应选中');
  assert.ok(!getEl('tab-catalog')._cls.has('sel'), '目录 Tab 不应选中');
});

test('市场视图目录 Tab：已安装排最前 + [已安装] 徽标', async () => {
  await load();
  await loadMarket();
  tab = 'catalog'; sel = null; render();
  const out = getEl('items')._html;
  const posMatch = out.indexOf('match');
  const posOw = out.indexOf('ow-recruit');
  const posZzz = out.indexOf('zzz-new-app');
  assert.ok(posMatch !== -1 && posZzz !== -1 && posMatch < posZzz, '已装 match 应排在未装 zzz 前');
  assert.ok(posOw !== -1 && posZzz !== -1 && posOw < posZzz, '已装 ow-recruit 应排在未装 zzz 前');
  assert.strictEqual((out.match(/\[已安装\]/g) || []).length, 2, '徽标应恰好 2 个');
  assert.ok(getEl('count')._text.includes('3 个可用'), '计数应为 3 个可用');
});

test('工作台：应用卡片网格渲染 + 状态条', async () => {
  await load();
  const out = getEl('wb-apps')._html;
  assert.ok(out.includes('match') && out.includes('ow-recruit'), '应用卡片应包含已安装应用');
  assert.ok(out.includes('data-run='), '卡片应带运行按钮');
  assert.ok(getEl('wb-funcs')._html.includes('知识图谱'), '常用功能应含知识图谱入口');
  assert.ok(getEl('wb-status')._html.includes('文档'), '状态条应显示');
});
