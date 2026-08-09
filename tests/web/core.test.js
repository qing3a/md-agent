#!/usr/bin/env node
/* md-agent 前端纯逻辑测试（node:test 内置 runner，零依赖）：
 * 运行: node --test tests/web/  （或 scripts/test.sh 一键）
 * 覆盖 web/core.js 全部导出：关键词提取/联网触发/工具 JSON/CE 组装器/会话解析/权限映射 */
const { test } = require('node:test');
const assert = require('node:assert');
const Core = require('../../web/core.js');

const TOOL_API = { search: 1, memory_search: 1, tasks: 1 };

test('extractKeywords：ASCII 词提取', () => {
  assert.deepStrictEqual(Core.extractKeywords('test rag vector'), ['test', 'rag', 'vector']);
});

test('extractKeywords：CJK 滑动双字', () => {
  assert.ok(Core.extractKeywords('向量检索').includes('向量'));
});

test('extractKeywords：功能词切断', () => {
  const ks = Core.extractKeywords('为什么这样');
  assert.ok(!ks.includes('为什么') && ks.includes('这样'));
});

test('extractKeywords：大小写归一 + 全角标点剥离', () => {
  assert.strictEqual(Core.extractKeywords('RAG System')[0], 'rag');
  const ks = Core.extractKeywords('向量，检索。完成！');
  assert.ok(ks.includes('向量') && ks.includes('检索'));
});

test('webTrigger：触发词命中/不误判', () => {
  assert.ok(Core.webTrigger('帮我搜一下最新的 Rust 版本'));
  assert.ok(Core.webTrigger('联网查一下 DeepSeek 定价'));
  assert.ok(Core.webTrigger('最新的 md-agent 版本是什么'));
  assert.ok(!Core.webTrigger('记忆统一模型怎么设计'));
  assert.ok(!Core.webTrigger(''));
});

test('extractJsonObjects：嵌套/括号/多对象', () => {
  assert.strictEqual(Core.extractJsonObjects('{"a":1}').length, 1);
  assert.strictEqual(Core.extractJsonObjects('{"tool":"x","args":{"q":"a{b}c"}}').length, 1);
  assert.strictEqual(Core.extractJsonObjects('{"q":"a{b}c"}')[0], '{"q":"a{b}c"}');
  assert.strictEqual(Core.extractJsonObjects('{"a":1}  {"b":2}').length, 2);
  assert.strictEqual(Core.extractJsonObjects('{"a":1').length, 0);
});

test('tryParseTool：平铺/包裹/多选一/未知工具', () => {
  const r1 = Core.tryParseTool('{"tool":"search","q":"x"}', TOOL_API);
  assert.strictEqual(r1.tool, 'search');
  const r2 = Core.tryParseTool('{"tool":"search","args":{"q":"x"}}', TOOL_API);
  assert.strictEqual(r2.args.q, 'x');
  const r3 = Core.tryParseTool('{"tool":"memory_search","q":"a"} {"tool":"search","q":"b"}', TOOL_API);
  assert.strictEqual(r3.tool, 'memory_search');
  assert.strictEqual(Core.tryParseTool('{"tool":"nope"}', TOOL_API), null);
  assert.strictEqual(Core.tryParseTool('正常回答', TOOL_API), null);
});

test('tryParseTool：工具名容错（DeepSeek 函数化调用式）', () => {
  assert.strictEqual(Core.tryParseTool('{"tool":"search()","q":"x"}', TOOL_API).tool, 'search');
  assert.strictEqual(Core.detectToolInFull('先查一下\n{"tool":"search()","q":"x"}', TOOL_API).tool, 'search');
  assert.strictEqual(Core.tryParseTool('{"tool":"nope()"}', TOOL_API), null);
});

test('detectToolInFull：前缀说明/长正文不误判', () => {
  assert.strictEqual(Core.detectToolInFull('{"tool":"search","q":"x"}', TOOL_API).tool, 'search');
  assert.strictEqual(Core.detectToolInFull('需要检索一下\n\n{"tool":"search","q":"x"}', TOOL_API).tool, 'search');
  assert.strictEqual(Core.detectToolInFull('这是一段很长的回答。'.repeat(50) + '{"tool":"search","q":"x"}', TOOL_API), null);
  assert.strictEqual(Core.detectToolInFull('正常回答', TOOL_API), null);
});

test('diffLines：LCS 差异行', () => {
  const d = Core.diffLines(['a', 'b', 'c'], ['a', 'x', 'c']);
  assert.strictEqual(d.filter((x) => x.t === '-').length, 1);
  assert.strictEqual(d.filter((x) => x.t === '+').length, 1);
  assert.strictEqual(d.filter((x) => x.t === ' ').length, 2);
});

test('CE 组装器：稳定前缀顺序 + today 注入 + 空 parts 过滤', () => {
  Core.resetSectionCache();
  const g = Core.buildGuidePrefix({ guideText: 'G1', memoryText: 'M1', toolsTxt: 'T1', today: '2026-08-03' });
  assert.ok(g.indexOf('【规范层') < g.indexOf('【记忆/索引层'));
  assert.ok(g.indexOf('【记忆/索引层') < g.indexOf('可用工具'));
  assert.ok(g.indexOf('可用工具') < g.indexOf('回答规则'));
  assert.ok(g.includes('今天是 2026-08-03'));
  Core.resetSectionCache();
  assert.ok(!Core.buildGuidePrefix({ guideText: 'G', toolsTxt: 'T', today: 'x' }).includes('【记忆/索引层'));
});

test('CE 组装器：分节注册表 memo/cacheBreak/reset/fresh', () => {
  Core.resetSectionCache();
  Core.buildGuidePrefix({ guideText: 'G1', memoryText: 'M1', toolsTxt: 'T1', today: '2026-08-03' });
  const m2 = Core.buildGuidePrefix({ guideText: 'G2', memoryText: 'M2', toolsTxt: 'T2', today: '2026-08-04' });
  assert.ok(m2.includes('G1') && !m2.includes('G2'), 'memo 节复用首值');
  assert.ok(m2.includes('今天是 2026-08-04'), 'cacheBreak 节每轮重算');
  Core.resetSectionCache();
  const m3 = Core.buildGuidePrefix({ guideText: 'G2', memoryText: 'M2', toolsTxt: 'T2', today: '2026-08-05' });
  assert.ok(m3.includes('G2') && !m3.includes('G1'), 'reset 后重算');
  const f1 = Core.buildGuidePrefix({ guideText: 'GG', memoryText: 'MM', toolsTxt: 'TT', today: 'x' }, { fresh: true });
  assert.ok(f1.includes('GG'), 'fresh 绕过缓存');
});

test('buildSkillTail：空/有技能', () => {
  assert.strictEqual(Core.buildSkillTail(''), '');
  assert.ok(Core.buildSkillTail('SK').includes('相关技能'));
  assert.ok(!Core.buildSkillTail('SK').includes('回答规则'), 'tail 独立片段');
});

test('parseSessionFile：frontmatter/旧格式/空/多行', () => {
  const sf = '---\ntype: session\ndate: 2026-08-05\ntitle: 测试\nstatus: active\ncount: 2\n---\n\n# 会话记录\n\n## Q: 问题一\nA: 回答一\n\n## Q: 问题二\nA: 回答二\n';
  const sp = Core.parseSessionFile(sf);
  assert.strictEqual(sp.length, 2);
  assert.strictEqual(sp[0].q, '问题一');
  assert.strictEqual(sp[1].a, '回答二');
  assert.strictEqual(Core.parseSessionFile('# 会话记录\n\n## Q: 旧问题\nA: 旧回答\n').length, 1);
  assert.strictEqual(Core.parseSessionFile('').length, 0);
  assert.strictEqual(Core.parseSessionFile('## Q: q\nA: 第一行\n第二行\n').length, 1);
});

test('isOverflowError：上下文超限命中/误判排除', () => {
  assert.ok(Core.isOverflowError("This model's maximum context length is 128000 tokens"));
  assert.ok(Core.isOverflowError('reduce the length of the messages or completion'));
  assert.ok(!Core.isOverflowError('未配置 LLM'));
  assert.ok(!Core.isOverflowError('HTTP 404'));
});

test('权限映射：llm/search/graph/file/storage + 管理端点拒绝', () => {
  assert.strictEqual(Core.permForPath('/api/llm'), 'llm');
  assert.strictEqual(Core.permForPath('/api/search?q=x'), 'search');
  assert.strictEqual(Core.permForPath('/api/graph/stats'), 'graph');
  assert.strictEqual(Core.permForPath('/api/file'), 'file');
  assert.strictEqual(Core.permForPath('/api/apps/match/data'), 'storage');
  assert.strictEqual(Core.permForPath('/api/config'), null);
  assert.strictEqual(Core.permForPath('/api/heartbeat'), null);
  assert.ok(Core.appCan('/api/llm', 'POST', ['llm']));
  assert.ok(!Core.appCan('/api/llm', 'POST', ['search']));
  assert.ok(Core.appCan('/api/file', 'GET', ['read']) && !Core.appCan('/api/file', 'POST', ['read']));
  assert.ok(Core.appCan('/api/file', 'POST', ['write']));
  assert.ok(Core.appCan('/api/apps/match/data', 'GET', ['storage']));
  assert.ok(!Core.appCan('/api/config', 'GET', ['llm', 'search', 'graph']));
});
