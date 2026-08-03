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

console.log('== CE 组装器 v1（稳定前缀在前） ==');
const g = Core.buildGuidePrefix({ guideText: 'G1', memoryText: 'M1', toolsTxt: 'T1', today: '2026-08-03' });
t('规范层在记忆层前', g.indexOf('【规范层') < g.indexOf('【记忆/索引层'));
t('记忆层在工具清单前', g.indexOf('【记忆/索引层') < g.indexOf('可用工具'));
t('工具清单在回答规则前', g.indexOf('可用工具') < g.indexOf('回答规则'));
t('today 注入', g.includes('今天是 2026-08-03'));
t('空 parts 过滤（无 memory）', !Core.buildGuidePrefix({ guideText: 'G', toolsTxt: 'T', today: 'x' }).includes('【记忆/索引层'));
t('无技能尾部为空', Core.buildSkillTail('') === '');
t('有技能尾部', Core.buildSkillTail('SK') === '相关技能（命中触发词，按技能步骤执行）：SK');
const full = [Core.buildGuidePrefix({ guideText: 'G', memoryText: 'M', toolsTxt: 'T', today: 'x' }), Core.buildSkillTail('SK')].filter(Boolean).join('\n\n');
t('技能在稳定前缀之后', full.indexOf('相关技能') > full.indexOf('回答规则'));

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
t('未映射端点拒绝', !Core.appCan('/api/config', 'GET', ['llm', 'search', 'graph']));

console.log('\n结果: ' + pass + ' 通过, ' + fail + ' 失败');
process.exit(fail ? 1 : 0);
