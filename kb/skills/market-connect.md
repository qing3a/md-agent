---
type: skill
title: 连接 SkillHub 商店
trigger: SkillHub
desc: 用户要求安装/连接第三方应用商店、应用市场、SkillHub 时，按步骤调用 market.connect
---
# 连接 SkillHub 商店

当用户要求「安装/连接某个应用商店、应用市场、SkillHub」并给出地址时，执行：

1. 提取 hub 索引 URL：用户给出完整 URL（形如 `.../install/skillhub.md`）直接用；若只给了域名（如 `skillhub.cn`），补全为 `https://<域名>/install/skillhub.md`
2. 调用工具 `market.connect`，参数 `hub_url` = 该 URL
3. 工具返回 hub 名与可用应用清单后，向用户汇报：已连接 SkillHub「<hub名>」（N 个应用），并列出应用 id
4. 用户若要求安装其中某应用，提示可执行 `/market install <id>`（人审确认）或在面板 `/view market` 的「目录」页操作
