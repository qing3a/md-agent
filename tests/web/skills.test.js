#!/usr/bin/env node
/* skills.html 面板逻辑测试（Node 桩环境跑真实脚本）：
 * 已装/商店分流 + 商店信息合并 + 排序（store 按下载量、分组按成员数）+ 卡片元信息
 * 运行: node --test tests/web/skills.test.js
 * 注：market.html 已改名 skills.html（c045c34），旧 market.test.js 失效，本文件为其替代 */
const { test } = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');

// ---- 最小 DOM 桩 ----
function makeEl(id) {
  const el = {
    id, _text: '', _html: '', _style: {}, _cls: new Set(), _attrs: {},
    style: { display: '' }, className: '', value: '',
    dataset: {}, addEventListener() {},
    set textContent(v) { this._text = String(v); },
    get textContent() { return this._text; },
    set innerHTML(v) { this._html = String(v); },
    get innerHTML() { return this._html; },
    setAttribute(k, v) { this._attrs[k] = String(v); },
    getAttribute(k) { return this._attrs[k]; },
  };
  el.classList = {
    toggle(c, on) { el._cls[on ? 'add' : 'delete'](c); },
    add(c) { el._cls.add(c); },
    remove(c) { el._cls.delete(c); },
    contains(c) { return el._cls.has(c); },
  };
  return el;
}
const els = {};
const getEl = (id) => (els[id] = els[id] || makeEl(id));
global.document = { getElementById: getEl, querySelectorAll: () => [], addEventListener: () => {} };
global.window = { parent: { postMessage() {} } };
global.prompt = () => null;

// 商店目录（catalog）：ow-recruit/match 未装；risk-check 已装（走注册表）
const catalogApps = [
  { id: 'ow-recruit', name: 'Ow Recruit 招聘工作台', version: '1.0', kind: 'app', category: 'professional', downloads: 120, description: '⬇ 120 · 招聘全流程' },
  { id: 'match', name: '相亲评估工作台', version: '0.2', kind: 'app', category: 'life-service', downloads: 80, description: '⬇ 80 · 相亲评估' },
  { id: 'zzz-new', name: '新技能', version: '0.1', kind: 'skill', category: 'other', downloads: 5, description: '⬇ 5 · 新条目' },
];
const skillsReg = [
  { name: 'case-risk.md', title: '案件风险', trigger: '风险', desc: '风控预警' },
  { name: 'market-connect.md', title: '市场连接', trigger: '' },
];
global.hostApi = (p) => Promise.resolve(
  p === '/api/hubs' ? { hubs: [{ id: 'h1', name: 'skillhub.cn' }] }
  : p === '/api/market/catalog' ? { apps: catalogApps }
  : p === '/api/skills' ? { skills: skillsReg } : {}
);

const html = fs.readFileSync(path.join(__dirname, '../../web/views/skills.html'), 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
eval(script);

test('build：已装/商店分流——注册表全进已装、目录未装进商店、计数正确', async () => {
  await load();
  assert.strictEqual(installed.length, 2, '注册表 2 项应全在已装');
  assert.strictEqual(store.length, 3, '目录 3 项未装应全进商店');
  assert.ok(installed.some((a) => a.id === 'case-risk' && a.fromLocal), '本地导入技能 fromLocal=true');
  assert.ok(installed.every((a) => !store.some((s) => s.id === a.id)), '已装不应出现在商店');
  assert.strictEqual(getEl('count')._text, '已装 2 · 商店 3');
});

test('build：商店命中已装条目 → 合并商店名称/版本', async () => {
  // 目录里加一个已装 id：market-connect 出现在商店 → 已装条目带上商店名与版本
  const extra = { id: 'market-connect', name: '市场连接（商店版）', version: '2.0', kind: 'skill', category: 'other', downloads: 9, description: '⬇ 9 · 连接市场' };
  catalogApps.push(extra);
  await load();
  const it = installed.find((a) => a.id === 'market-connect');
  assert.ok(it, 'market-connect 应仍在已装');
  assert.strictEqual(it.name, '市场连接（商店版）', '应合并商店名称');
  assert.strictEqual(it.version, '2.0', '应合并商店版本');
  assert.ok(!store.some((s) => s.id === 'market-connect'), '商店命中已装 → 不应进商店列');
  catalogApps.pop();
});

test('build + render：商店按下载量降序；已装按中文名排序', async () => {
  await load();
  assert.deepStrictEqual(store.map((a) => a.id), ['ow-recruit', 'match', 'zzz-new'], '商店按 downloads 降序');
  const names = installed.map((a) => a.name);
  assert.deepStrictEqual(names, [...names].sort((a, b) => a.localeCompare(b, 'zh')), '已装按中文名排序');
  const htmlOut = getEl('inst-items')._html;
  assert.ok(htmlOut.includes('case-risk') && htmlOut.includes('风险'), '已装列表渲染 id/触发词');
});

test('render：商店按 category 分组，组按成员数降序', async () => {
  // 4 个条目：professional×2（ow-recruit + 加一个）→ professional 组最大排最前
  const extra = { id: 'headhunter', name: '猎头助手', version: '0.5', kind: 'app', category: 'professional', downloads: 200, description: '⬇ 200 · 猎头' };
  catalogApps.push(extra);
  await load();
  const out = getEl('store-items')._html;
  const profLabel = '专业服务';
  const profPos = out.indexOf(profLabel);
  const lifePos = out.indexOf('生活服务');
  const otherPos = out.indexOf('其他');
  assert.ok(profPos !== -1 && lifePos !== -1 && otherPos !== -1, '三个分组都应渲染');
  assert.ok(profPos < lifePos && profPos < otherPos, '成员最多的 professional 组应排最前');
  assert.ok(out.indexOf('⬇ 200') !== -1, '卡片底部显示下载量');
  assert.ok(out.indexOf('⬇ 200') < out.indexOf('⬇ 120'), '下载量降序的卡片顺序');
  catalogApps.pop();
});

test('cardHtml：下载量从描述剥离，描述无 ⬇ 前缀', () => {
  const c = cardHtml({ id: 'x', name: 'X', kind: 'skill', version: '1.1', category: 'other', description: '⬇ 42 · 真正的描述' }, 0);
  assert.strictEqual(c.match(/真正的描述/g).length, 2, '描述出现于 title 与正文（无 ⬇ 前缀）');
  assert.ok(!c.includes('⬇ 42 ·'), '⬇ 前缀应已剥离');
  assert.strictEqual(c.match(/⬇ 42/g).length, 1, '下载量只保留在底栏');
});

test('itemHtml：kind 徽标（应用/技能）+ 触发词', () => {
  const app = itemHtml({ id: 'match', name: '相亲', kind: 'app' }, 0, 'inst');
  const sk = itemHtml({ id: 'case-risk', name: '案件风险', kind: 'skill', trigger: '风险' }, 1, 'inst');
  assert.ok(app.includes('应用') && !app.includes('技能'), 'app 徽标');
  assert.ok(sk.includes('技能') && !sk.includes('应用'), 'skill 徽标');
  assert.ok(sk.includes('触发: 风险'), '触发词显示');
});
