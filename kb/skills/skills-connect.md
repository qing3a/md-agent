---
type: skill
title: 连接技能商店
trigger: SkillHub
desc: 用户要求安装/连接第三方应用商店、应用市场、SkillHub、技能商店时，按步骤调用 skills.connect
---
# 连接技能商店

当用户要求「安装/连接某个应用商店、应用市场、SkillHub、技能商店」并给出地址时，执行：

1. 提取 hub 来源：用户给出完整地址（形如 `https://skillhub.cn/install/skillhub.md`、`git+https://github.com/user/skills-repo`、`local:C:/path/to/skills`）直接用；若只给了域名（如 `skillhub.cn`），补全为 `https://<域名>/install/skillhub.md`
2. 调用工具 `skills.connect`，参数 `hub_url` = 该地址
3. 工具返回 hub 名与可用技能/应用清单后，向用户汇报：已连接 SkillHub「<hub名>」（N 个技能/应用），并列出条目 id
4. 用户若要求安装其中某技能或应用，提示可执行 `/skills install <id>`（人审确认）或在侧边栏「🧩 技能」面板的「目录」页操作；用户找技能但不知道名字时，用 `skills.search` 检索