# chrome-agent

[![Crates.io](https://img.shields.io/crates/v/chrome-agent)](https://crates.io/crates/chrome-agent)
[![npm](https://img.shields.io/npm/v/chrome-agent)](https://www.npmjs.com/package/chrome-agent)
[![CI](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/sderosiaux/chrome-agent/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024_edition-orange)](https://doc.rust-lang.org/edition-guide/rust-2024/)

<p align="center">
  <img src="docs/hero-logo.png" alt="chrome-agent — 面向 AI Agent 的浏览器自动化" width="500">
</p>

<p align="center">
  <a href="README.md">English</a> | <a href="README.cn.md">简体中文</a>
</p>

面向 AI Agent 的浏览器自动化。一个 3 MB 的 Rust 二进制文件通过 CDP 驱动 Chrome——不需要 Node，不需要
Playwright，不需要守护进程。每一次操作都会报告页面是否真的照做了，输出是 Agent 可以直接分支判断的 JSON。

> 独立项目。与 Google 或 Chrome 团队无关联、无背书、无赞助。

## 安装

```bash
npx skills add sderosiaux/chrome-agent   # 技能文件 + 二进制，供编码 Agent 使用
npm install -g chrome-agent              # 预编译二进制
cargo install chrome-agent               # 从源码安装
```

Linux 版本是静态 musl 二进制，任何发行版都能运行，不需要匹配 glibc 版本。

## 60 秒上手

```bash
# 导航，并把页面读成带稳定 uid 的无障碍树
chrome-agent goto https://example.com --inspect
# uid=n9  heading "Example Domain" level=1
# uid=n12 link "More information..."

# 三种定位方式：uid、CSS 选择器、坐标
chrome-agent click n12 --inspect
chrome-agent click --selector "button.submit"
chrome-agent click --xy 100,200

# 填写，然后检查页面实际保留了什么
chrome-agent fill --uid n20 "user@test.com"
chrome-agent assert value --uid n20 --equals "user@test.com"

# 拿内容，而不是标记
chrome-agent read
chrome-agent extract --limit 30
chrome-agent text --selector "main" --truncate 500

# 一切都可以是 JSON
chrome-agent --json eval "document.title"
```

Chrome 在两次调用之间保持存活，所以一条命令的开销是一次连接，而不是一次浏览器启动。用
`--browser <name>` 给每个并行 Agent 各自的 Chrome 和各自的会话状态。

## 命令

### 导航与会话

| 命令 | 作用 |
|---|---|
| `goto <url> [--inspect] [--max-depth N] [--header "K: V"]` | 导航。自动补 `https://`。返回 `landed`（见下文）。`--header` 可重复。 |
| `back [--inspect]` | 后退。返回 `url` 与 `title`；无处可退时返回一条消息。 |
| `forward [--inspect]` | 前进。同一实现，符号相反。 |
| `history [--filter pattern]` | 该浏览器访问过的页面。 |
| `tabs` | 列出打开的标签页。 |
| `status` | 会话库中的浏览器、它们的 pid，以及没有条目认领的运行中实例（`orphan=`）。 |
| `close [--purge] [--orphans]` | 关闭浏览器。`--purge` 删除 cookie 和配置目录。`--orphans` 关闭无人认领的实例。 |

### 读取页面

| 命令 | 作用 |
|---|---|
| `inspect [--verbose] [--max-depth N] [--uid nN] [--filter "role,role"] [--scroll] [--limit N] [--urls] [--max-chars N] [--offset K]` | 带 uid 的无障碍树。`--urls` 解析链接 href。收窄类参数只影响**打印**内容；存下来的基线始终是完整树。 |
| `diff` | 自上次 inspect 以来的变化，与那棵完整基线树对比。 |
| `text [uid] [--selector "css"] [--truncate N]` | 整页或单个元素的可见文本。 |
| `read [--html] [--truncate N]` | 用 Mozilla Readability 提取正文。 |
| `extract [--selector "css"] [--limit N] [--scroll] [--a11y]` | 自动识别重复记录（商品、信息流、搜索结果），无需写选择器。React SPA 用 `--a11y`。 |
| `eval <expression> [--selector "css"]` | 在页面上下文执行 JS。`el` 是匹配到的元素。 |
| `screenshot [--filename name] [--format jpeg\|png] [--quality N] [--max-width N] [--uid nN\|--selector "css"]` | 截图到文件路径。`--uid`/`--selector` 裁剪到单个元素。 |
| `pdf [--filename name] [--landscape] [--background]` | 把页面打印成 PDF。 |
| `download <url> [--out path] [--timeout N] [--max-bytes N]` | 在页面内 fetch，因此 cookie 和登录态照常生效。也可以用 `download --uid nN` / `--selector "css"` 点击并捕获浏览器原生下载。 |

### 操作

| 命令 | 作用 |
|---|---|
| `click <uid> [--selector "css"] [--xy X,Y] [--inspect]` | 点击。没有 box model 时回退到 JS `.click()`。 |
| `dblclick <uid>` | 双击，同样三种定位方式。 |
| `fill --uid <uid> <value> [--secret] [--inspect]` | 填写输入框，也支持 `--selector "css"`。会报告页面实际保留的值；`--secret` 只报告长度。 |
| `fill-form <uid=val>...` | 一次填多个字段，每个字段各有一份保留值报告。 |
| `select --uid <uid> <value>` | 按 value 或可见文本选择 `<select>` 选项。 |
| `check <uid>` | 确保复选框或单选框被勾选。幂等。 |
| `uncheck <uid>` | 确保复选框被取消勾选。幂等。 |
| `upload --uid <uid> <file>...` | 上传到文件输入框。路径先校验。 |
| `drag <from-uid> <to-uid>` | 基于鼠标事件的拖拽。不适用于 HTML5 Drag and Drop API。 |
| `type <text> [--selector "css"] [--secret]` | 向获得焦点的元素输入文本。`--secret` 连长度也不报告。 |
| `press <key>` | Enter、Tab、Escape 等。 |
| `scroll <down\|up\|uid>` | 滚动页面，或把某个元素滚入视口。 |
| `hover <uid>` | 悬停。 |
| `wait <text\|url\|selector> <pattern>` | 等待某个条件。 |
| `wait network-idle [--idle-ms N] [--timeout N]` | 等到 `--idle-ms`（默认 500）内没有请求在途。 |

### 校验

| 命令 | 作用 |
|---|---|
| `assert value (--selector "css"\|--uid nN) (--equals\|--contains\|--matches) <s>` | 表单控件的值。密文会比较但绝不打印。 |
| `assert text (--contains\|--matches) <s> [--selector "css"\|--uid nN]` | 整页或单个元素的可见文本。 |
| `assert url (--equals\|--matches) <s>` | 当前 URL。 |
| `assert state (--selector "css"\|--uid nN) (--checked\|--unchecked\|--selected <opt>\|--enabled\|--disabled\|--visible)` | 勾选状态、当前选项、禁用状态或是否渲染。 |
| `assert exists --selector "css" [--count N\|--min N]` | 匹配到多少个元素。`--count 0` 断言不存在。 |

### 监控与进阶

| 命令 | 作用 |
|---|---|
| `network [--filter "pattern"] [--body] [--live N] [--abort "pattern"]` | 请求与 API 响应。`--abort` 在 `--live N` 秒内拦截匹配的请求。 |
| `console [--level error] [--clear]` | console.log/warn/error 与 JS 异常。 |
| `frame <selector\|main>` | 把 `eval`/`inspect` 绑定到 iframe。仅在同一个 `pipe`/`batch` 进程内有效。 |
| `emulate device --width W --height H [--dpr N] [--mobile] [--touch] [--orientation portrait\|landscape] [--label name]` | 为某个具名页面设置设备参数。另有 `emulate status` 和 `emulate reset`。 |
| `webmcp list` | 页面注册在 `document.modelContext` 上的工具。另有 `webmcp call <name> --args '{"k":"v"}'`。 |
| `macro record <name> --from-recording <file>` | 把一次录制的会话提炼成带守卫、可传参的路径。另有 `macro list`、`macro show`、`macro run`。 |
| `replay <file>` | 逐条重放一个 `pipe --record` 文件。 |
| `batch` | 从 stdin 读取一个 JSON 数组并依次执行。 |
| `pipe` | 常驻的 JSON stdin/stdout 连接。 |

## 全局参数

```
--browser <name>         具名浏览器配置（默认 "default"）
--page <name>            具名标签页（默认 "default"）
--connect <auto|url>     接管一个正在运行的 Chrome（必须带值）
--proxy-server <url>     为托管的 Chrome 设置代理（http(s)、socks4/5；端口需显式）
--headed                 显示浏览器窗口（默认无头）
--stealth                反检测 CDP 补丁
--copy-cookies           使用你真实 Chrome 配置里的 cookie
--chrome-arg <flag>      传给被启动 Chrome 的额外参数（可重复）
--timeout <seconds>      命令超时（默认 30）
--max-depth <N>          限制 inspect 深度
--verdict <mode>         auto（默认）回读页面；off 只报告动作本身
--budget <chars>         限制变更报告的长度（默认 1200；0 表示不限）
--on-intercept <mode>    dispatch（默认）、guard 或 refuse
--ignore-https-errors    接受自签名证书
--dialog <mode>          JS 对话框策略：accept（默认）、dismiss 或 manual
--dialog-text <text>     --dialog accept 下提交给 prompt() 的文本
--json                   结构化 JSON 输出
```

全局参数放在动词前后都能解析。`--timeout` 和 `--max-depth` 是例外：自己声明了这两个参数的命令
（`wait`、`download`，以及所有带 `--inspect` 的命令）要写在动词之后，其他情况写在动词之前。
`--proxy-server` 和 `--chrome-arg` 只在启动时生效，并且在一个具名浏览器的生命周期内固定——之后省略它们
的命令会继承，指定不同值的命令会被拒绝。要改就先 close 或 purge 该浏览器。

## 概念

### uid

元素 id 来自 Chrome 的 `backendNodeId`，打印成 `n82`。同一个页面多次 inspect 之间保持有效，所以 Agent
可以 inspect 一次、操作多次。导航会重新分配全部 uid——`goto`、`back` 或触发路由变化的点击之后要重新
inspect。`goto` 清空 uid 映射但保留快照，所以 `diff` 会报 `document_changed` 而不是报错。需要解析 uid
的命令必须拿到**已存储**快照里的 uid：`assert value --uid` 或 `download --uid` 之前先 inspect。

### verdict：页面到底照做了没有

`ok:true` 只表示命令跑完了，不表示页面照做了。每个会改变页面的操作都带 `verdict`、`verdict_reason`
和 `next`——`next` 是一个闭集里的六个 token 之一，调用方无需解析散文即可分支。

| `verdict` | `verdict_reason` | `next` | 含义 |
|---|---|---|---|
| `changed` | `tree_delta`, `nodes_moved`, `focus_only` | `proceed` | 页面动了；`delta` 说明怎么动的。`focus_only` 表示唯一的变化是焦点落到了一个真实元素上，而那可能是你所点元素的可聚焦祖先。 |
| `changed` | `value_kept` | `proceed` / `inspect` | 元素上的回读确认了这次写入，而树无法体现（密文字段渲染成固定标记）。页面读取同时失败时，`next` 是 `inspect`。 |
| `changed` | `values_lost` | `confirm` | 页面动了**并且**清空了一个原本有值的字段，`values_lost` 逐个列出。提交后自动清空的表单，和把输入丢掉的表单，看起来完全一样。 |
| `navigated` | `document_replaced` | `inspect` | 新文档。所有存下来的 uid 都失效了。 |
| `intercepted` | `hit_test_receiver`, `modal_dialog` | `dismiss` | 另一个元素占据了那个点并接收了事件，`intercepted_by` 会指名它（tag、id、class、uid、z-index）。关于目标元素一无所知。 |
| `not_kept` | `value_reverted`, `value_rewritten` | `stop` | 写入到达了元素而元素没有保留：要么为空，要么被掩码改写。读 `value.actual`；再填一次结果相同。 |
| `no_effect` | `delivered_no_change` | `confirm` | 命中测试证明了投递，且树在 `observed_after_ms` 内没有动。 |
| `unchanged` | `identical_tree` | `confirm` | 观察期间树完全相同。投递未被证明。 |
| `unknown` | `no_baseline`, `read_failed`, `identity_unreadable` | `inspect` | 无法比较。绝不等于「什么都没发生」。 |
| `unknown` | `aim_point_off_target` | `inspect` | 什么都没派发，且两次读取的瞄准点一致，所以重试会以同样的方式落空。 |
| `unknown` | `scroll_not_settled` | `retry` | 什么都没派发，且两次读取不一致，所以重试不会重复任何动作。 |
| `not_checked` | `reporting_disabled` | `proceed` | 你传了 `--verdict off`。 |

`unknown` 时绝不要重复操作：第一次可能已经生效了。

指针类操作还会报告 `delivery`（`target_hit`、`intercepted`、`off_target`、`not_settled`、`js`、
`not_probed`），来自即将派发坐标上的一次命中测试。只有在 `target_hit` 之后才可能给出 `no_effect`。
`--on-intercept` 决定被别的东西挡住时怎么办：

| `--on-intercept` | 行为 |
|---|---|
| `dispatch`（默认） | 始终把事件透过接收者派发出去。 |
| `guard` | 接收者是惰性的（没有可交互标签或角色、不可聚焦、没有 `cursor: pointer`）就派发；它可能是个控件、是 `<iframe>`、或无法识别时拒绝。 |
| `refuse` | 从不派发。返回 `ok:false`、退出码 1，并带上 `delivery`、`intercepted_by`、`verdict`、`next` 和 `dispatched:false`。 |

两个盲区：回读窗口固定为 60 毫秒（报告为 `observed_after_ms`，所以 400 毫秒才触发的校验器落在窗口之外
——请用 `wait` 加 `assert value`）；canvas、WebGL 和纯 CSS 效果对无障碍树不可见。

### `landed` 与 `serving`：你落在哪里，以及是谁应答的

`goto` 返回 `landed{requested,final,redirected,http_status,serving}`。仅片段或末尾斜杠的差异不算重定向。
`http_status` 是最后一跳的状态码，取自 Navigation Timing API，因此不影响 `--stealth`。

`serving` 永远不会改变 `ok` 或退出码。请对 `serving` 分支，而不是对 `ok`：

| `serving` | 含义 |
|---|---|
| `page` | 没有任何测量结果与「页面已加载」相矛盾。这是证据的缺席，不是证书——付费墙同样读作 `page`。 |
| `challenge` | 页面上有反爬厂商的 frame 或脚本，而没有站点自己的表单。`challenge_from` 指出其域名。请用 `--connect`，而不是 `--stealth`。 |
| `error` | 服务器返回 4xx/5xx，`http_status` 说明具体是哪一个。 |
| `nothing_actionable` | 没有链接、没有表单控件、没有脚本、几乎没有文本。尚未渲染完成的页面也是这个样子。放弃之前先跑 `inspect`。 |
| `unreadable` | 形态探针没有运行。 |

### 退出码

`0` 成功 · `1` 错误，包括参数写错 · `2` 本工具做出的某个断言不成立 · `130` Ctrl+C。`2` 只有两个来源：
`assert`，以及 `macro run` 里被检查过却不成立的 guard——这两者都是本工具对页面许下的承诺，所以 CI
能区分「页面不对」和「工具坏了」。

```bash
chrome-agent fill --selector "#coupon" "SAVE10"
chrome-agent assert value --selector "#coupon" --equals "SAVE10"
chrome-agent assert state --selector "#terms" --checked
chrome-agent assert exists --selector ".result" --min 1
```

`assert` 是一次读取：没有变更报告、没有 verdict，也从不点击任何东西。`--matches` 是 Rust 正则
（`\d`/`\w`/`\s` 仅限 ASCII，没有 `\p{...}`；`(?i)` 可用）。在 `batch` 和 `pipe` 里断言本身没有退出码——它
表现为 `ok:false` 加一个 `assertion` 对象；因它而中止的 `batch` 退出 `1`，不是 `2`。

### pipe 与 batch 模式

一个进程、一条连接、每个响应一行 JSON，而且整段序列里 uid 保持稳定——后者才是选它的理由。加速是真的，但很小：
pipe 省掉的是每条命令约 12 ms 的固定开销，**一串读取快 1.5 倍**（九条命令，352 ms → 228 ms），
**一串填写与点击只快 1.1 倍**（2029 ms → 1908 ms）——那里的大头是 pipe 碰不到的沉降窗口和树的重读。
测量于 2026-08-30，M4 Max，Chrome 152，9 次运行取中位数；用 `./scripts/measure-pipe.sh` 复现，
记录在 `docs/design/pipe-latency.md`。

```bash
echo '{"cmd":"goto","url":"https://example.com","inspect":true}
{"cmd":"click","uid":"n12","inspect":true}
{"cmd":"read"}' | chrome-agent pipe
```

`batch` 改为从 stdin 读一个 JSON 数组，其余走同一套分发逻辑，并且只回一个包含全部结果的响应对象，而不是
每条命令一行。加 `--json` 才会把它打印成 JSON；不加时 CLI 每条结果打印一行文本。

当 `--stop-on-error` 中途截断整批时，CLI 的 `batch` 进程退出 `1`——绝不会是 `2`：进程是在报告这一批停下了，
而不是在对页面做任何主张，`2` 只留给「本工具做出的主张不成立」。
不加 `--stop-on-error` 时它执行完了被交代的每条命令，即使其中一条失败也退出 `0`：请读 `ok`，批次上的和每条
结果上的。

### iframe

`frame` 把 `eval` 和 `inspect` 绑定到某个 iframe。这个绑定存在于连接上，所以只在一个 `pipe` 或 `batch`
进程内部有效：

```bash
printf '%s\n' \
  '{"cmd":"frame","target":"#payment-iframe"}' \
  '{"cmd":"inspect"}' \
  '{"cmd":"fill","uid":"n42","value":"4242424242424242"}' \
  '{"cmd":"frame","target":"main"}' | chrome-agent pipe
```

请精确指定 iframe（`iframe[src*="checkout"]`）；裸写 `iframe` 匹配的是 DOM 顺序里的第一个，往往是个空广告位。
`frame` 不会作用于 `--selector` 定位——切换后先 inspect，再按 uid 操作，uid 是跨 frame 有效的。隔离世界能
看到该 frame 的 DOM，但看不到它主世界的 JS 变量。

### `--stealth` 与 `--connect`

`--stealth` 应用 7 个 CDP 层面的补丁（`Page.addScriptToEvaluateOnNewDocument`），不是 Chrome 启动参数：
`navigator.webdriver`、`chrome.runtime`、Permissions API、WebGL renderer、User-Agent、一个输入坐标泄漏，
以及始终不调用 `Runtime.enable`。

| 防护 | 有效手段 |
|---|---|
| 无 | `chrome-agent goto ...` |
| Cloudflare JS 挑战（"Just a moment…"） | `--stealth` 能过 |
| Cloudflare 托管 Turnstile | `--stealth` 无效，请用 `--connect`。 |
| DataDome、Kasada | `--stealth` 无效，请用 `--connect`。 |
| 需要登录的站点 | `--copy-cookies`，可搭配 `--stealth` |

重度防护指纹识别的是 Chromium 二进制本身，所以唯一出路是真实安装的 Chrome。`--copy-cookies` 复制你 Chrome
配置里的 cookie 数据库；两个实例用同一个 Keychain，所以加密 cookie 也能用，而你真实的 Chrome 不受影响。

```bash
google-chrome --remote-debugging-port=9222 &
chrome-agent --connect http://127.0.0.1:9222 goto https://www.leboncoin.fr --inspect
chrome-agent --stealth --copy-cookies goto x.com/home --inspect
chrome-agent --copy-cookies goto github.com/notifications --inspect
```

### 宏

宏是一条已经成功过的路径，连同那次成功时观察到的后置条件一起以名字保存下来。`macro run` 会检查每一步的
守卫，并在第一个不成立的地方停下。没有修复，也没有重试。

```bash
chrome-agent macro record cancel --from-recording session.jsonl
chrome-agent macro run cancel --var email=ada@example.com
```

守卫是 `delivery: target_hit`、verdict 词、`value.verbatim`，以及一条由路径构造的 `url_matches`——绝不包括
变更计数、uid 或耗时。按 uid 瞄准的步骤会以角色加无障碍名称记录，否则被拒绝。密文字段变成声明的参数，
绝不写入文件。

一个被检查过却不成立的守卫，退出码是 **2**——和断言失败同一个码，因为它们是同一类主张。报告里带
`stopped_by: "guard"`，以及是哪个守卫、期望什么、实际看到什么。因其他原因停下的运行——步骤本身失败、
页面读不出来、宏文件不存在——退出 `1`，带 `stopped_by: "error"`。

### 落盘文件

截图、PDF 和下载文件落在 `~/.chrome-agent/tmp`（或你的 `--out` 路径），权限 `0600`，路径打印到 stdout。
二进制字节绝不进入 stdout。

```bash
chrome-agent download https://app.com/reports/2024.csv --out ./2024.csv
chrome-agent download --selector "#export" --out ./export.csv
chrome-agent pdf --filename invoice.pdf --background
chrome-agent screenshot --uid n42
```

要读 `downloaded`，不是 `ok`。一次已投递但没产生文件的点击返回 `ok:true` 加 `downloaded:false`：点击不可
撤销，在那里报错只会诱使你再真点一次。`--timeout` 限定整个窗口；`--max-bytes` 会取消超出限制的传输。

### 设备模拟

```bash
chrome-agent --page mobile emulate device --label "checkout phone" \
  --width 412 --height 915 --dpr 2.625 --mobile --touch
chrome-agent --page mobile emulate status
chrome-agent --page mobile emulate reset
```

设备参数属于某个具名页面，并在每次连接开始时重新应用，因为设置它的 CDP 会话一断开，Chrome 就会丢弃所有
覆盖值。`--touch` 下 `click` 和 `check` 派发触摸点击；`dblclick`、`hover` 和 `drag` 仍是鼠标事件。参数值
必须显式给出：设备预设目录活在 DevTools 前端里，不在 CDP 里。

### WebMCP

页面可以在 `document.modelContext` 上注册工具（W3C WICG WebMCP）。协议没有定义 `outputSchema`，所以
`webmcp call` 会把工具自己声明的结果，和页面可测量的实际变化并排报告，用的是与其他所有操作相同的 verdict
机制。

```bash
chrome-agent webmcp list
chrome-agent inspect
chrome-agent webmcp call add_to_cart --args '{"item":"Espresso Blend"}'
```

大多数已安装的 Chrome 没有原生 WebMCP。用 `--chrome-arg --enable-features=WebMCP,WebMCPTesting` 对真实
API 测试，或者对一个自带 polyfill 的页面测试。在 `frame` 绑定下响应会带 `frame_scoped: true`：由该 frame
主世界脚本注册的工具，从隔离世界里是看不见的。

## 排障

| 现象 | 原因 | 处理 |
|---|---|---|
| `Element uid=nN not found` | 页面导航过，或者快照根本没存过。 | 重新跑 `inspect`。 |
| 并行 Agent 互相污染 | 它们共用了 `--browser default`。 | 给每个 Agent 一个 `--browser <unique>`。 |
| `frame` 切换丢失 | 每次 CLI 调用都是一条新连接。 | 改用 `pipe` 或 `batch` 驱动。 |
| `select` 报 "Element is not a `<select>`" | 那是自定义的 React/MUI 下拉。 | 先点开，再点选项。 |
| 明明生效的点击 verdict 却是 `unchanged` | canvas、纯 CSS 或延迟效果对树不可见。 | 先 `wait`，再 `assert`。 |
| 带 `--stealth` 仍然 403 或出验证码 | 托管 Turnstile、DataDome 或 Kasada。 | `--connect` 到真实 Chrome。 |
| 明明会渲染的页面报 `serving: nothing_actionable` | 稳定探针在首帧之前就停了。 | 跑一次 `inspect`。 |
| 卡在原生 `alert`/`confirm` 上 | 你传了 `--dialog manual`。 | 去掉它，或者自己应答对话框。 |
| `network --abort` 什么都没拦到 | 它是阻塞式的，只运行 `--live N` 秒。 | 在导航之前就启动它。 |

## 对比

|  | chrome-agent | agent-browser (Vercel) | Playwright MCP |
|---|---|---|---|
| 语言 | Rust | Rust | TypeScript |
| 体积 | 3 MB，零运行时 | 3 MB CLI + 面板 + 云服务商 | Node + Playwright |
| 启动 | 实测 12 ms，浏览器已在跑时的一条命令 | 守护进程（首次之后快） | 冷启动 |
| UID 稳定性 | `backendNodeId`，多次 inspect 之间稳定 | 顺序 `@e1`，每次快照重新分配 | 无（用选择器） |
| 操作 + 观察 | `--inspect` 参数，一次调用 | 另外调一次快照 | 另外调一次 |
| 合规性报告 | 每个操作都有 `verdict`/`next` | 无 | 无 |
| 反检测 | 7 个 CDP 补丁 | 交给云服务商 | 无 |
| 阅读模式 | `read`（Readability.js） | 无 | 无 |
| 记录抽取 | `extract`，结构化，不调用 LLM | 无 | 无 |
| PDF 导出 | `pdf` | 无 | 无 |
| MCP server | 无 | 有 | 有 |
| 云服务商、iOS/Safari | 无（可 `--connect` 到任何东西） | 有 | 无 |
| 代码量 | ~28.2K 行 Rust 代码（src/ 下，不含空行与纯注释行；有测试重新测量） | ~40K 行（他们的数字，此处未核实） | Playwright |

`extract` 用 MDR/DEPTA 风格的启发式（兄弟节点相似度、内容异质性、文本/链接比）在结构上找出重复记录，而不是
让模型去读 DOM。在 Hacker News 首页，它用 1,571 tokens 交出 30 条记录，无障碍树要 5,652，原始 HTML 要
8,727（[`scripts/measure.sh`](scripts/measure.sh)）。差距取决于页面：在一个通篇只有列表的博客归档页上，两者
相差无几。

## 什么时候不该用它

- 你需要一个测试框架——用 Playwright。
- 你需要 MCP server——这里没有。
- 你需要浏览器集群、代理池或验证码破解——去看 Browserbase、Steel、Browserless。
- 你需要 Firefox 或 Safari——这个工具说的是 CDP，只支持 Chrome。
- 你想要一个有支持保障的产品——这是一个人的项目。

## 从 Agent 里使用

`npx skills add sderosiaux/chrome-agent` 会装一个 SKILL.md。否则 `chrome-agent --help` 里内嵌了完整的 LLM
使用指南，而且每个错误都带一个指明下一步的 `hint`。Claude Code 权限配置：
`{"permissions": {"allow": ["Bash(chrome-agent *)"]}}`。

```
chrome-agent（3 MB Rust 二进制，~28.2K 行 Rust 代码（src/ 下））
    | CDP over WebSocket
    v
Chrome（默认无头，无 Node.js，无运行时）
```

设计记录见 [`docs/design/README.md`](docs/design/README.md)。

## 许可证

MIT
