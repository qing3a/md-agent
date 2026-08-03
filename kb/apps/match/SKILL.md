---
type: skill
name: match
title: 相亲评估工作台
trigger: 相亲|匹配|评估
version: 0.2.0
entry: index.html
permissions: [llm]
desc: 候选人评估工作台（HTML 型 app 包）：md-agent 内以应用运行（沙箱 iframe + llm 权限走桥），纯指令 agent 可当技能用
---
# 相亲评估工作台

单文件 HTML 交互应用（rose 品牌色）：候选人资料录入 → 三维评分 → 心理画像 / 红旗扫描 → 看板多视图跟进。

- **md-agent 中**：`/view match` 以应用运行（沙箱 iframe，权限白名单只放行 `llm`）
- **其他 agent 中**：本文件即技能说明，按下方步骤辅助评估

## 使用步骤

1. 收集候选人资料（年龄 / 职业 / 家庭 / 性格等维度），录入工作台
2. 按三维度（适配度 / 风险 / 长期潜力）打分并记录理由
3. 输出心理画像，执行红旗扫描
4. 汇总综合评估与跟进建议，沉淀到知识库
