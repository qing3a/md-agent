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

console.log('\n结果: ' + pass + ' 通过, ' + fail + ' 失败');
process.exit(fail ? 1 : 0);
