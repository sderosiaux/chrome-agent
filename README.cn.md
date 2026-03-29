# chrome-agent

[![Crates.io](https://img.shields.io/crates/v/chrome-agent)](https://crates.io/crates/chrome-agent)
[![npm](https://img.shields.io/npm/v/chrome-agent)](https://www.npmjs.com/package/chrome-agent)
[![CI](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024_edition-orange)](https://doc.rust-lang.org/edition-guide/rust-2024/)

<p align="center">
  <img src="docs/hero-logo.png" alt="chrome-agent — Browser automation for AI agents" width="500">
</p>

<p align="center">
  <strong>让 LLM 驾驭浏览器。</strong>
</p>

<p align="center">
  <a href="README.md">English</a> | <a href="README.cn.md">简体中文</a>
</p>

> **免责声明：** 这是一个独立的社区驱动项目，与 Google 或 Chrome 团队没有任何关联、认可或赞助关系。

> 用户不是你，是你的 LLM。
>
> 你不需要读这份 README，你的 Agent 才需要。安装后运行 `chrome-agent --help`，让 LLM 自己搞定。CLI 内嵌了完整的使用指南，每条错误都附带下一步操作提示，`--json` 模式输出结构化数据，Agent 无需任何适配器即可解析。这个页面只是因为 GitHub 需要一个。

Playwright 返回 2,000 个 token 的原始 HTML。chrome-agent 返回 50 个 token 的无障碍树，并带有稳定的元素 ID。无需编写 CSS 选择器，无需解析 DOM。

```bash
chrome-agent goto news.ycombinator.com --inspect

# ~50 个 token，而非 ~2,000：
uid=n1 RootWebArea "Hacker News"
  uid=n50 heading "Hacker News" level=1
  uid=n82 link "Show HN: A New Browser Tool"
  uid=n97 link "Rust 2025 Edition Announced"
  ...

# 点击 + 一次调用查看新页面：
chrome-agent click n82 --inspect
```

UID 基于 Chrome 的 `backendNodeId`，在多次 inspect 之间保持不变。现在点击 `n82`，或者五分钟后再点击，都没问题。

```
chrome-agent（3 MB Rust 二进制文件）
    | CDP over WebSocket
    v
Chrome（无头模式，无 Node.js，无运行时依赖）
```

### 为什么做这个

| 如果你遇到了这些问题... | chrome-agent 的解决方式 |
|---|---|
| Playwright 快照消耗 2K token | 无障碍树：约 50 个 token。页面状态的上下文消耗减少 40 倍。 |
| CSS 选择器每次部署都会失效 | 基于 Chrome `backendNodeId` 的 UID，只要 DOM 节点存在就保持稳定。 |
| 点击后再 inspect = 2 次往返 | 任意命令加 `--inspect`，一次调用完成操作 + 观察。 |
| 200MB 的 Node + npm + Playwright | 3 MB 二进制文件。`npx chrome-agent` 开箱即用。 |
| Cloudflare 拦截无头 Chrome | 7 项 CDP 补丁。`Runtime.enable` 从不被调用（没人提到的检测手段）。 |
| 为每个网站编写抓取选择器 | `read` 提取文章，`extract` 提取列表/表格/卡片，`network` 获取 API 响应。无需选择器。 |
| 错误信息是堆栈跟踪 | `{"ok":false, "error":"...", "hint":"run inspect"}` — 可解析、可操作。 |
| 每条命令都启动新浏览器 | 会话持久化。Chrome 在调用之间保持运行。启动约 10ms。 |
| Agent 无法访问已登录的账号 | `--copy-cookies` 从真实 Chrome 获取 cookie。支持 X.com、Gmail、各种后台面板。 |
| 无限滚动只显示 10 条 | `inspect --scroll --limit 50` 滚动并收集。已在 X.com 测试：从实时时间线获取 50 条推文。 |
| 两个 Agent 共享一个浏览器 = 混乱 | `--browser agent1`、`--browser agent2`，独立的 Chrome 实例。 |

## 安装

```bash
# 为 AI Agent 安装 -- 会安装一个 SKILL.md，你的 Agent 会自动读取
npx skills add sderosiaux/chrome-agent

# 或者只安装二进制文件
npm install -g chrome-agent    # 预编译版
npx chrome-agent --help        # 免安装
cargo install chrome-agent     # 从源码编译
```

## 快速上手

```bash
# 导航并查看页面
chrome-agent goto https://example.com --inspect

# 通过 uid 点击
chrome-agent click n12 --inspect

# 填写表单
chrome-agent fill --uid n20 "user@test.com"

# CSS 选择器同样可用
chrome-agent click --selector "button.submit"
chrome-agent fill --selector "input[name=email]" "hello@test.com"

# 文章内容（Readability — 类似 Firefox 阅读模式）
chrome-agent read

# 可见文本，限定范围并截断
chrome-agent text --selector "main" --truncate 500

# 执行 JS
chrome-agent eval "document.title"

# 截图（返回文件路径，非二进制数据）
chrome-agent screenshot
```

## 命令列表

| 命令 | 功能 |
|---------|------------|
| `goto <url> [--inspect] [--max-depth N]` | 导航。缺少 `https://` 时自动补全。 |
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"] [--scroll] [--limit N]` | 带 UID 的无障碍树。`--scroll --limit` 用于无限滚动。 |
| `click <uid> [--inspect]` | 通过 uid 点击。无盒模型时回退到 JS `.click()`。 |
| `click --selector "css" [--inspect]` | 通过 CSS 选择器点击。 |
| `click --xy 100,200` | 通过坐标点击。 |
| `fill --uid <uid> <value> [--inspect]` | 通过 uid 填写输入框。 |
| `fill --selector "css" <value>` | 通过选择器填写。 |
| `fill-form <uid=val>...` | 批量填写。 |
| `read [--html] [--truncate N]` | 通过 Mozilla Readability 提取文章。 |
| `text [uid] [--selector "css"] [--truncate N]` | 获取页面或元素的可见文本。 |
| `eval <expression> [--selector "css"]` | 在页面上下文中执行 JS。`el` = 匹配的元素。 |
| `extract [--selector "css"] [--limit N] [--scroll] [--a11y]` | 自动检测重复数据。`--a11y` 用于 React SPA（如 X.com）。 |
| `network [--filter "pattern"] [--body] [--live N]` | 网络请求和 API 响应。 |
| `console [--level error] [--clear]` | console.log/warn/error + JS 异常。 |
| `pipe` | 持久化 JSON stdin/stdout 连接。 |
| `wait <text\|url\|selector> <pattern>` | 等待条件满足。 |
| `type <text> [--selector "css"]` | 在聚焦元素中输入文本。 |
| `press <key>` | Enter、Tab、Escape 等按键。 |
| `scroll <down\|up\|uid>` | 滚动页面或将元素滚动到可见区域。 |
| `hover <uid>` | 悬停。 |
| `back` | 浏览器后退。 |
| `screenshot [--filename name]` | 截图保存到文件。 |
| `tabs` | 列出打开的标签页。 |
| `diff` | 查看上次 inspect 以来的变化。 |
| `close [--purge]` | 停止浏览器。`--purge` 删除 cookie/配置。 |
| `status` | 会话信息。 |
| `stop` | 停止守护进程。 |

## 全局参数

```
--browser <name>         命名浏览器配置（默认："default"）
--page <name>            命名标签页（默认："default"）
--connect [url]          连接到正在运行的 Chrome
--headed                 显示浏览器窗口（默认：无头模式）
--stealth                反检测补丁（Cloudflare、Turnstile）
--copy-cookies           使用真实 Chrome 配置的 cookie
--timeout <seconds>      命令超时时间（默认：30）
--max-depth <N>          限制 inspect 深度
--ignore-https-errors    接受自签名证书
--json                   结构化 JSON 输出
```

## 核心循环：inspect、操作、inspect

```bash
chrome-agent goto https://app.com/login --inspect
# uid=n52 textbox "Email" focusable
# uid=n58 textbox "Password" focusable
# uid=n63 button "Sign In" focusable

chrome-agent fill --uid n52 "user@test.com"
chrome-agent fill --uid n58 "password123"
chrome-agent click n63 --inspect
# uid=n101 heading "Dashboard" level=1
```

只要 DOM 节点存在，UID 在多次 inspect 之间保持不变。

## 内容提取

按 token 消耗从少到多排列：

```bash
# 文章（Readability，类似 Firefox 阅读模式）
chrome-agent read

# 重复数据 -- 商品、搜索结果、信息流。无需选择器。
chrome-agent extract
# 使用 MDR/DEPTA 启发式算法，自动发现数据模式。

# React SPA（X.com 等）-- 使用无障碍树替代 DOM
chrome-agent extract --a11y --scroll --limit 20

# 限定范围的可见文本
chrome-agent text --selector "[role=main]" --truncate 1000

# API 响应 -- 跳过 DOM
chrome-agent network --filter "api" --body
```

## 隐身模式

`--stealth` 通过 CDP 修补 7 项自动化指纹：

- `navigator.webdriver` 设为 `undefined`
- `chrome.runtime` 模拟
- Permissions API 修复
- WebGL 渲染器掩码
- User-Agent 清理
- 输入坐标泄漏补丁
- `Runtime.enable` 从不调用

这些是 CDP 层面的补丁（`Page.addScriptToEvaluateOnNewDocument`），不是 Chrome 启动参数。

对于使用更强保护（DataDome、Kasada）且会检测 Chromium 二进制文件指纹的网站，请连接到真实的 Chrome：

```bash
google-chrome --remote-debugging-port=9222 &
chrome-agent --connect http://127.0.0.1:9222 goto https://www.leboncoin.fr --inspect
```

| 防护等级 | 解决方案 |
|---|---|
| 无防护 | `chrome-agent goto ...` |
| Cloudflare/Turnstile | `chrome-agent --stealth goto ...` |
| 需要登录的网站 | `chrome-agent --stealth --copy-cookies goto ...` |
| DataDome/Kasada | `chrome-agent --connect` 连接到真实 Chrome |

## 已登录网站

`--copy-cookies` 从你的 Chrome 配置中复制 cookie 数据库。两个 Chrome 实例使用同一个 macOS Keychain，因此加密的 cookie 直接可用。

```bash
chrome-agent --stealth --copy-cookies goto x.com/home --inspect
# 你的时间线、你的私信，无需登录流程。

chrome-agent --copy-cookies goto mail.google.com --inspect
chrome-agent --copy-cookies goto github.com/notifications --inspect
```

你的真实 Chrome 不受影响。

## 网络和控制台捕获

```bash
# 已加载的资源（隐身安全，使用 Performance API）
chrome-agent network --filter "api"

# 实时流量及响应体
chrome-agent network --live 5 --body --filter "graphql"

# 控制台输出
chrome-agent console --level error    # 仅错误 + 异常
```

控制台捕获使用注入的拦截器，而非 `Runtime.enable`。

## Pipe 模式

对于需要连续发送多条命令的 Agent，pipe 模式保持单一连接：

```bash
echo '{"cmd":"goto","url":"https://example.com","inspect":true}
{"cmd":"click","uid":"n12","inspect":true}
{"cmd":"read"}' | chrome-agent pipe
```

每条响应一行 JSON。比每条命令启动一个进程快约 10 倍。

## JSON 模式

```bash
chrome-agent --json goto https://example.com --inspect
# {"ok":true,"url":"...","title":"...","snapshot":"uid=n1 heading..."}

chrome-agent --json eval "1+1"
# {"ok":true,"result":2}

# 错误也返回退出码 0，确保 Agent 始终能解析 stdout：
chrome-agent --json click n99
# {"ok":false,"error":"Element uid=n99 not found.","hint":"Run 'chrome-agent inspect'"}
```

## 多标签页与并行 Agent

```bash
# 同一浏览器中的多个标签页
chrome-agent --page main goto https://app.com
chrome-agent --page docs goto https://docs.app.com
chrome-agent --page main eval "document.title"   # "App"

# 多个 Agent，各自拥有独立的 Chrome
chrome-agent --browser agent1 goto https://example.com
chrome-agent --browser agent2 goto https://other.com
```

## 与 AI Agent 配合使用

```bash
# 安装技能文件（Claude Code、Cursor、Copilot 等）
npx skills add sderosiaux/chrome-agent

# 或者让你的 Agent 运行：
chrome-agent --help
# 帮助输出包含完整的 LLM 使用指南。
```

Claude Code 权限配置：

```json
{
  "permissions": {
    "allow": ["Bash(chrome-agent *)"]
  }
}
```

## 对比

| | chrome-agent | dev-browser | chrome-devtools-mcp | Playwright MCP |
|---|---|---|---|---|
| 语言 | Rust | Rust + Node.js | TypeScript | TypeScript |
| 运行时依赖 | 无 | Node + npm + Playwright + QuickJS | Node + Puppeteer | Node + Playwright |
| 二进制大小 | 3 MB | 3 MB CLI + 200 MB 依赖 | npm 包 | npm 包 |
| CLI 启动速度 | ~10ms（会话复用） | ~500ms | 不适用（MCP） | 不适用（MCP） |
| 元素定位 | uid + 选择器 + 坐标 | 选择器 + snapshotForAI | uid（顺序分配） | 选择器 |
| UID 稳定性 | backendNodeId（稳定） | 不适用 | 顺序分配（会重新编号） | 不适用 |
| 操作 + 观察 | `--inspect`（1 次调用） | 批量脚本 | 每次操作 1 次调用 | 每次操作 1 次调用 |
| 隐身模式 | 7 项 CDP 补丁 | 无 | 无 | 无 |
| 阅读模式 | `read`（Readability） | 无 | 无 | 无 |
| 网络捕获 | 回溯 + 实时 | 无 | 无 | 仅元数据 |
| 数据提取 | `extract`（自动检测） | 无 | 无 | 无 |
| 控制台捕获 | 隐身安全 | 无 | 有 | 无 |
| Pipe 模式 | 有 | 无 | 无 | 无 |
| 代码量 | ~6.2K 行 | ~76K 行 | ~12K 行 | Playwright |

## 许可证

MIT

> 本文档由社区维护。如有翻译问题，欢迎提交 PR。
