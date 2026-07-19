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

## 和 agent-browser 有什么不同？

[agent-browser](https://github.com/vercel-labs/agent-browser)（Vercel）是一个功能完整的浏览器自动化平台：仪表盘、云服务商、标注截图、iOS 支持、AI 对话、凭证保险库，40K 行 Rust。它很优秀。

chrome-agent 是相反的策略：不是增加功能，而是减少 token。

| | chrome-agent | agent-browser |
|---|---|---|
| **页面快照** | ~50 token（无障碍树噪音过滤，减少 66%） | ~200 token（完整无障碍树） |
| **元素 ID** | `backendNodeId` — 跨 inspect 稳定 | 顺序 `@e1, @e2` — 每次快照重新分配 |
| **操作 + 观察** | `click n12 --inspect`（1 次调用） | `click @e1` 然后 `snapshot`（2 次调用） |
| **隐身模式** | 7 项原生 CDP 补丁（含 `Runtime.enable` 跳过） | 委托给云服务商 |
| **内容提取** | `read`（文章）、`extract`（自动检测列表/表格） | 无内置功能 |
| **二进制** | 3 MB，零运行时依赖 | 3 MB + Next.js 仪表盘 + 云 SDK |
| **代码量** | ~8.8K 行 | 40K 行 |

agent-browser 提供带监控、云浏览器和可视化调试的平台。chrome-agent 给你的 LLM 提供最精简的网页表示，然后退出舞台。如果你的 Agent 需要仪表盘，用 agent-browser。如果你的 Agent 需要把 token 花在推理而不是解析页面上，用这个。

## 设计理念

Agent 花在理解页面上的每一个 token，都是它无法用来思考任务的 token。chrome-agent 围绕一个核心理念构建：**最小化从"这个页面长什么样？"到"我下一步该做什么？"之间的 token 消耗。**

具体来说：

- **无障碍树优于 DOM。** Playwright 返回 ~2,000 token 的原始 HTML。chrome-agent 返回 ~50 token 的无障碍树，带有稳定的元素 ID。无需 CSS 选择器，无需解析 DOM。
- **单一二进制，零运行时。** 3 MB Rust 二进制。无 Node.js，无 npm，无 Playwright 运行时。`npx chrome-agent` 直接可用。
- **一次调用完成操作 + 观察。** 任意操作命令加 `--inspect` 就能返回操作后的页面状态。一次往返而非两次。
- **错误即指令。** 每个错误都包含 `hint` 字段，告诉 Agent 下一步做什么。`{"ok":false, "error":"...", "hint":"run inspect"}`。
- **隐身优先。** 7 项 CDP 补丁，包含没人提到的检测手段（`Runtime.enable`）。最强防护场景可连接到真实 Chrome。
- **无需选择器的内容提取。** `read` 提取文章，`extract` 提取重复数据，`network` 获取 API 响应。Agent 永远不需要写 CSS 选择器。

这不是通用的浏览器测试框架，而是让 LLM 高效浏览网页的工具。

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

### 导航

| 命令 | 功能 |
|---------|------------|
| `goto <url> [--inspect] [--max-depth N] [--header "K: V"]` | 导航。缺少 `https://` 时自动补全。`--header`（可重复）发送额外的 HTTP 请求头。 |
| `back` | 浏览器后退。 |
| `forward` | 浏览器前进。 |
| `close [--purge]` | 停止浏览器。`--purge` 删除 cookie/配置。 |

### 检查

| 命令 | 功能 |
|---------|------------|
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"] [--scroll] [--limit N] [--urls] [--max-chars N] [--offset K]` | 带 UID 的无障碍树。`--scroll --limit` 用于无限滚动。`--urls` 解析链接 href。`--max-chars`/`--offset` 限制并分页输出。 |
| `diff` | 查看上次 inspect 以来的变化。 |
| `screenshot [--filename name] [--format jpeg\|png] [--quality N] [--max-width N] [--uid nN\|--selector "css"]` | 截图保存到文件。JPEG/quality/max-width 缩小体积；`--uid`/`--selector` 裁剪到单个元素。 |
| `pdf [--filename name] [--landscape] [--background]` | 将当前页面打印为 PDF 文件。 |
| `tabs` | 列出打开的标签页。 |

### 交互

| 命令 | 功能 |
|---------|------------|
| `click <uid> [--inspect]` | 通过 uid 点击。无盒模型时回退到 JS `.click()`。 |
| `click --selector "css" [--inspect]` | 通过 CSS 选择器点击。 |
| `click --xy 100,200` | 通过坐标点击。 |
| `dblclick <uid> [--inspect]` | 双击。同样支持 `--selector`、`--xy`。 |
| `fill --uid <uid> <value> [--inspect]` | 通过 uid 填写输入框。 |
| `fill --selector "css" <value>` | 通过选择器填写。 |
| `fill-form <uid=val>...` | 批量填写。 |
| `select --uid <uid> <value>` | 按值或可见文本选择下拉选项。 |
| `select --selector "css" <value>` | 通过 CSS 选择器选择。 |
| `check <uid>` | 确保复选框/单选框为选中状态（幂等）。 |
| `uncheck <uid>` | 确保复选框/单选框为未选中状态（幂等）。 |
| `upload --uid <uid> <file>...` | 上传文件到文件输入框。 |
| `upload --selector "css" <file>...` | 通过 CSS 选择器上传。 |
| `drag <from-uid> <to-uid>` | 拖拽元素到另一个元素。 |
| `type <text> [--selector "css"]` | 在聚焦元素中输入文本。 |
| `press <key>` | Enter、Tab、Escape 等按键。 |
| `scroll <down\|up\|uid>` | 滚动页面或将元素滚动到可见区域。 |
| `hover <uid>` | 悬停。 |
| `wait <text\|url\|selector> <pattern>` | 等待条件满足。 |
| `wait network-idle [--idle-ms N] [--timeout N]` | 等待网络静默 `--idle-ms`（默认 500）后返回。比固定 sleep 更适合 SPA/XHR 稳定。 |

### 内容提取

| 命令 | 功能 |
|---------|------------|
| `read [--html] [--truncate N]` | 通过 Mozilla Readability 提取文章。 |
| `text [uid] [--selector "css"] [--truncate N]` | 获取页面或元素的可见文本。 |
| `eval <expression> [--selector "css"]` | 在页面上下文中执行 JS。`el` = 匹配的元素。 |
| `extract [--selector "css"] [--limit N] [--scroll] [--a11y]` | 自动检测重复数据。`--a11y` 用于 React SPA（如 X.com）。 |
| `download <url> [--out path] [--timeout N]` | 在页面内 fetch 下载 URL，因此 cookie/登录态自动带上（可下载需登录的文件）。返回 `{path,bytes,mime}`。 |

### 监控

| 命令 | 功能 |
|---------|------------|
| `network [--filter "pattern"] [--body] [--live N] [--abort "pattern"]` | 网络请求和 API 响应。`--abort` 拦截匹配的请求。 |
| `console [--level error] [--clear]` | console.log/warn/error + JS 异常。 |

### 高级

| 命令 | 功能 |
|---------|------------|
| `frame <selector\|main>` | 将 `eval`/`inspect` 切换进 iframe（或切回主页面）。仅在单个 `pipe`/`batch` 进程内持续有效。 |
| `batch` | 从 stdin 的 JSON 数组执行多条命令。 |
| `pipe` | 持久化 JSON stdin/stdout 连接。 |

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
--dialog <mode>          JS 对话框策略：accept（默认）、dismiss 或 manual
--dialog-text <text>     当 --dialog accept 时，为 prompt() 对话框提交的文本
```

JS 对话框（`alert`/`confirm`/`prompt`/`beforeunload`）默认自动应答（`--dialog accept`）—— 否则原生对话框会阻塞页面且没有任何 DOM 信号，Agent 的下一条命令会挂起。使用 `--dialog dismiss` 取消它们，或用 `--dialog manual` 退出自动应答。

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

## 表单：下拉菜单、复选框、文件上传

```bash
# 按值或可见文本选择下拉选项
chrome-agent select --uid n15 "California"

# 幂等的复选框控制
chrome-agent check n20     # 已选中则不操作
chrome-agent uncheck n20   # 已取消选中则不操作

# 文件上传
chrome-agent upload --uid n30 /path/to/document.pdf

# 双击（文本选择、特殊控件）
chrome-agent dblclick n42
```

## iframe

`frame` 切换会把 `eval` 和 `inspect` 绑定到该 iframe —— 但**只在同一个进程内有效**，所以要通过 `pipe`（或 `batch`）驱动，绝不能用分开的 CLI 调用：

```bash
printf '%s\n' \
  '{"cmd":"frame","target":"#payment-iframe"}' \
  '{"cmd":"inspect"}' \
  '{"cmd":"fill","uid":"n42","value":"4242424242424242"}' \
  '{"cmd":"frame","target":"main"}' | chrome-agent pipe
```

- 精确指定目标 iframe（例如 `iframe[src*="checkout"]`）；裸写 `iframe` 会匹配 DOM 顺序中的第一个，往往是广告的 `about:blank` 槽位。
- `frame` 只作用于 `eval`/`inspect`，**不**作用于 `--selector` 定位。切换后先 `inspect` 拿到 iframe 内的 uid，再按 uid 操作（uid 跨 frame 均可解析）。
- 每个独立的 `chrome-agent <cmd>` 都会打开一个全新连接，因此 `chrome-agent frame …` 后再单独执行 `chrome-agent inspect` 会丢失切换状态。请使用 `pipe`/`batch`。

## 批量模式

从 stdin 的 JSON 数组执行命令序列，无需每条命令单独启动进程：

```bash
echo '[
  {"cmd":"goto","url":"https://example.com"},
  {"cmd":"inspect","filter":"button"},
  {"cmd":"click","uid":"n42"}
]' | chrome-agent batch
```

每条命令输出一行 JSON。比每条命令单独启动进程快约 10 倍。

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

## 网络捕获与请求拦截

```bash
# 已加载的资源（隐身安全，使用 Performance API）
chrome-agent network --filter "api"

# 实时流量及响应体
chrome-agent network --live 5 --body --filter "graphql"

# 拦截追踪/广告请求（使用 Fetch 域拦截）
chrome-agent network --abort "*tracking*" --live 30

# 控制台输出
chrome-agent console --level error    # 仅错误 + 异常
```

控制台捕获使用注入的拦截器，而非 `Runtime.enable`。

## 带链接 URL 的 inspect

Agent 在决定点击哪个链接时，通常需要 URL 而不仅仅是文本：

```bash
chrome-agent inspect --urls --filter link
# uid=n82 link "Pricing" url="https://example.com/pricing"
# uid=n97 link "Docs" url="https://docs.example.com"
```

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

# 错误返回退出码 1，但 JSON 仍在 stdout（可解析）：
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

|  | chrome-agent | agent-browser (Vercel) | Playwright MCP |
|---|---|---|---|
| 语言 | Rust | Rust | TypeScript |
| 二进制 | 3 MB，零运行时 | 3 MB CLI + 仪表盘 + 云服务商 | Node + Playwright |
| 启动速度 | ~10ms（会话复用） | 守护进程（首次后快速） | 冷启动 |
| Token 效率 | ~50 token/页（无障碍树噪音过滤） | ~200 token/页（无障碍树） | ~2,000 token（HTML） |
| UID 稳定性 | `backendNodeId`（跨 inspect 稳定） | 顺序 `@e1, @e2`（每次快照重新分配） | 不适用（选择器） |
| 操作 + 观察 | `--inspect` 参数（1 次调用） | 单独 snapshot 调用 | 单独调用 |
| 隐身 | 7 项原生 CDP 补丁 | 委托给云服务商 | 无 |
| 阅读模式 | `read`（Readability.js） | 无 | 无 |
| 数据提取 | `extract`（自动检测重复数据） | 无 | 无 |
| 代码量 | ~8.8K 行 | ~40K 行 | Playwright |
| 设计目标 | 最少 token，最大自主性 | 功能完整平台 | 浏览器测试 |

## 许可证

MIT

> 本文档由社区维护。如有翻译问题，欢迎提交 PR。
