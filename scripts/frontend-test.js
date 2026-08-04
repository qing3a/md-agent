#!/usr/bin/env node
/* md-agent 前端纯逻辑测试：直接 require web/core.js（Node 环境），零 DOM。
 * 运行: node scripts/frontend-test.js（或 scripts/test.sh 一键） */
const Core = require('../web/core.js');

let pass = 0, fail = 0;
function t(label, cond) {
  if (cond) { pass++; console.log('  PASS ' + label); }
  else { fail++; console.log('  FAIL ' + label); }
}

const TOOL_API = { search: 1, memory_search: 1, tasks: 1 };

console.log('== extractKeywords ==');
t('ASCII 词提取', JSON.stringify(Core.extractKeywords('test rag vector')) === '["test","rag","vector"]');
t('CJK 整段+滑动双字（向量）', Core.extractKeywords('向量检索').includes('向量'));
t('功能词切断（为什么）', !Core.extractKeywords('为什么这样').includes('为什么') && Core.extractKeywords('为什么这样').includes('这样'));
t('英文大小写归一', Core.extractKeywords('RAG System')[0] === 'rag');
t('全角标点剥离（，。！）', JSON.stringify(Core.extractKeywords('向量，检索。完成！')).includes('向量') && JSON.stringify(Core.extractKeywords('向量，检索。完成！')).includes('检索'));
t('全角括号不混入短语', Core.extractKeywords('知识库（整理）').includes('知识库') && Core.extractKeywords('知识库（整理）').includes('整理'));

console.log('== extractJsonObjects ==');
t('单对象', Core.extractJsonObjects('{"a":1}').length === 1);
t('嵌套 args', Core.extractJsonObjects('{"tool":"x","args":{"q":"a{b}c"}}').length === 1);
t('字符串含大括号不误切', Core.extractJsonObjects('{"q":"a{b}c"}')[0] === '{"q":"a{b}c"}');
t('多个空格分隔', Core.extractJsonObjects('{"a":1}  {"b":2}').length === 2);
t('未闭合放弃', Core.extractJsonObjects('{"a":1').length === 0);

console.log('== tryParseTool ==');
t('平铺参数', (r = Core.tryParseTool('{"tool":"search","q":"x"}', TOOL_API)) && r.tool === 'search' && r.args.q === 'x');
t('args 包裹', (r = Core.tryParseTool('{"tool":"search","args":{"q":"x"}}', TOOL_API)) && r.args.q === 'x');
t('多 JSON 取第一个合法', (r = Core.tryParseTool('{"tool":"memory_search","q":"a"} {"tool":"search","q":"b"}', TOOL_API)) && r.tool === 'memory_search');
t('未知工具返回 null', Core.tryParseTool('{"tool":"nope"}', TOOL_API) === null);
t('非 JSON 开头 null', Core.tryParseTool('正常回答', TOOL_API) === null);

console.log('== detectToolInFull ==');
t('纯 JSON 开头', (r = Core.detectToolInFull('{"tool":"search","q":"x"}', TOOL_API)) && r.tool === 'search');
t('先说明再调工具（短前缀）', (r = Core.detectToolInFull('需要检索一下\n\n{"tool":"search","q":"x"}', TOOL_API)) && r.tool === 'search');
t('长正文不误判', Core.detectToolInFull('这是一段很长的回答。'.repeat(50) + '{"tool":"search","q":"x"}', TOOL_API) === null);
t('无工具 JSON null', Core.detectToolInFull('正常回答', TOOL_API) === null);

console.log('== diffLines ==');
const d = Core.diffLines(['a', 'b', 'c'], ['a', 'x', 'c']);
t('LCS 差异行', d.filter((x) => x.t === '-').length === 1 && d.filter((x) => x.t === '+').length === 1);
t('相同行保留', d.filter((x) => x.t === ' ').length === 2);

console.log('== CE 组装器 v1（稳定前缀在前，分节注册表 memo） ==');
const freshPrefix = (parts) => { Core.resetSectionCache(); return Core.buildGuidePrefix(parts); };
const g = freshPrefix({ guideText: 'G1', memoryText: 'M1', toolsTxt: 'T1', today: '2026-08-03' });
t('规范层在记忆层前', g.indexOf('【规范层') < g.indexOf('【记忆/索引层'));
t('记忆层在工具清单前', g.indexOf('【记忆/索引层') < g.indexOf('可用工具'));
t('工具清单在回答规则前', g.indexOf('可用工具') < g.indexOf('回答规则'));
t('today 注入', g.includes('今天是 2026-08-03'));
t('空 parts 过滤（无 memory）', !freshPrefix({ guideText: 'G', toolsTxt: 'T', today: 'x' }).includes('【记忆/索引层'));
t('无技能尾部为空', Core.buildSkillTail('') === '');
t('有技能尾部', Core.buildSkillTail('SK') === '相关技能（命中触发词，按技能步骤执行）：SK');
// 技能移出 system（tail attachment 语义，cc-haha 背书）：稳定前缀不含技能内容——命中技能不炸前缀缓存
const gFull = freshPrefix({ guideText: 'G', memoryText: 'M', toolsTxt: 'T', today: 'x' });
t('稳定前缀不含技能内容', !gFull.includes('相关技能'));
t('技能 tail 为独立片段', !Core.buildSkillTail('SK').includes('回答规则'));

console.log('== 分节注册表契约（方向 3，cc-haha 语义） ==');
Core.resetSectionCache();
const m1 = Core.buildGuidePrefix({ guideText: 'G1', memoryText: 'M1', toolsTxt: 'T1', today: '2026-08-03' });
// 同会话第二次：guide/memory/tools 走 memo（返回首值）；rules 为 cacheBreak（today 每轮重算）
const m2 = Core.buildGuidePrefix({ guideText: 'G2', memoryText: 'M2', toolsTxt: 'T2', today: '2026-08-04' });
t('memo 节复用首值（guide 不更新）', m2.includes('G1') && !m2.includes('G2'));
t('memo 节复用首值（tools 不更新）', m2.includes('T1') && !m2.includes('T2'));
t('cacheBreak 节每轮重算（today 更新）', m2.includes('今天是 2026-08-04'));
// /clear 语义：resetSectionCache 后重算
Core.resetSectionCache();
const m3 = Core.buildGuidePrefix({ guideText: 'G2', memoryText: 'M2', toolsTxt: 'T2', today: '2026-08-05' });
t('reset 后重算（guide 更新）', m3.includes('G2') && !m3.includes('G1'));
t('reset 后重算（today 更新）', m3.includes('今天是 2026-08-05'));
// fresh 绕过缓存：fresh-window 降级用（显式丢记忆的最小上下文，不污染 memo 缓存）
const f1 = Core.buildGuidePrefix({ guideText: 'GG', memoryText: 'MM', toolsTxt: 'TT', today: 'x' }, { fresh: true });
t('fresh 绕过缓存（含新值）', f1.includes('GG'));
t('fresh 最小上下文（可丢 memory）', !Core.buildGuidePrefix({ guideText: 'GG', memoryText: '', toolsTxt: 'TT', today: 'x' }, { fresh: true }).includes('【记忆/索引层'));
// 方向 4：frc 指令节（工具结果为截断摘要、可重查）在稳定前缀内且为 memo 节
const f2 = freshPrefix({ guideText: '', memoryText: '', toolsTxt: 'T', today: 'x' });
t('frc 指令节在稳定前缀内', f2.includes('重新调用该工具') && f2.includes('截断摘要'));

console.log('== CE isOverflowError（fresh-window 降级触发） ==');
t('maximum context 命中', Core.isOverflowError('This model\'s maximum context length is 128000 tokens'));
t('token 超限命中', Core.isOverflowError('reduce the length of the messages or completion'));
t('未配置不误判', !Core.isOverflowError('未配置 LLM'));
t('404 不误判', !Core.isOverflowError('HTTP 404'));

console.log('== 应用市场权限映射（阶段 1） ==');
t('llm 映射', Core.permForPath('/api/llm') === 'llm');
t('search 映射（含 query）', Core.permForPath('/api/search?q=x') === 'search');
t('graph 映射', Core.permForPath('/api/graph/stats') === 'graph');
t('file 映射', Core.permForPath('/api/file') === 'file');
t('管理端点默认拒绝', Core.permForPath('/api/config') === null && Core.permForPath('/api/heartbeat') === null);
t('llm 权限放行', Core.appCan('/api/llm', 'POST', ['llm']));
t('无权限拒绝', !Core.appCan('/api/llm', 'POST', ['search']));
t('file GET=read', Core.appCan('/api/file', 'GET', ['read']) && !Core.appCan('/api/file', 'POST', ['read']));
t('file POST=write', Core.appCan('/api/file', 'POST', ['write']));
t('storage 映射（app data）', Core.permForPath('/api/apps/match/data') === 'storage');
t('storage 权限放行', Core.appCan('/api/apps/match/data', 'GET', ['storage']) && Core.appCan('/api/apps/match/data', 'POST', ['storage']));
t('无 storage 权限拒绝', !Core.appCan('/api/apps/match/data', 'POST', ['llm']));
t('未映射端点拒绝', !Core.appCan('/api/config', 'GET', ['llm', 'search', 'graph']));

console.log('\n结果: ' + pass + ' 通过, ' + fail + ' 失败');
process.exit(fail ? 1 : 0);
