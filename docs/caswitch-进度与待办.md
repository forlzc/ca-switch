# caswitch 精简重构：进度与收尾记录

> 更新时间：2026-11-06（GMT+8）
> 仓库：`/workspace/cc-switch`　远端：`https://github.com/forlzc/ca-switch.git`
> 分支：`ca-switch`
> 最终口径：**单 API 入口、无任何具体客户端功能、通用图标**
> 工作区：收尾完成后干净

---

## 1. 一句话现状

**项目已完成通用 API 代理收敛。** 后端 `AppType` 只剩 `Claude` 一个变体，前端只剩一个显示为「API」的入口；
Claude 插件、onboarding、官方账号额度、模型映射、Stack、编辑器投影、Claude Desktop、Prompt/CLAUDE.md、
MCP/Skills 槽位均已删除。13 个其他 app 的配置模块与命令已清除，5 张下线表通过 schema v28 迁移 DROP，
后端 `dead_code` 从 175 条降到 0。当前后端 `cargo check` 0 error / 0 warning，`cargo test` 全通过；
前端 `pnpm typecheck`、`pnpm vitest run`、`pnpm build:renderer` 均通过。

> 原 §6.2 的「前端四入口」方案已被最终口径取代：只保留一个 API 入口，协议由供应商侧 `apiFormat` 下拉选择。
> 详见 §11。
>
> 2026-11-06 追加：协议矩阵补齐（Chat×Anthropic）与真实上游验证，详见 §12；该轮改动已提交推送。
> 同日追加：断流修复、模型覆盖、配置随协议切换、UA 预设与模型刷新，并发布 v4.0.25，详见 §13。

---

## 2. 分支与提交清单

| 提交 | 阶段 | 内容 |
|---|---|---|
| `cee21206` | 基线后 | provider-form 自定义 User-Agent 预设分组展示 |
| `5aa2dacd` | P1 | 移除 MCP 与 Skills 全套支持 |
| `6c0a82af` | 2a | 引入 `Protocol` 枚举与 `Matrix::plan` 转换矩阵（与 `AppType` 并存编译，行为零变化） |
| `f316c24f` | 2b-1 | `infer::of_provider` 四级协议推断证据链 |
| `9ad93d6a` | 2e-1 | 代理侧摘除 Stack 路由 |
| `af275fb0` | 2e-2 | 切换收敛为全局单激活指针（InUse / Direct 合并） |
| `66912c95` | 2e-3a | 代理侧与 `mode` 解耦，统一走 `ProviderService::active_provider(db, app)` |
| `760b7dd5` | 2e-3b | 新增供应商只入库 |
| `cb693617` | 2e-3c | 编辑 / 删除只操作库，激活项不可删 |
| `3d1096f2` | 2e-3 + 2d | 移除 `live/`、`mode/` 与客户端配置管理面（84 文件 / −30977 行） |
| `851f7c58` | 2b-2 | schema v25：协议化 + 全局单激活迁移 |
| `b6b5d195` | 2c | 补齐 Chat×Anthropic 转换与协议矩阵真实验证 |
| `09835778` | 构建 | 修复 Windows 目标无法编译 |
| `ad576f56` | 修复 | Chat×Responses 长推理断流心跳、UA 预设、模型列表刷新 |
| `a684854a` | 修复 | 配置变量随上游协议切换、Anthropic 入口模型覆盖、用量删「应用」列 |
| `2ea72fce` | 发布 | 仓库更名为 ca-switch，同步代码内链接，重发 v4.0.25 |

**安全网**

- tag `caswitch-base`：重构前基线。
- 分支 `caswitch-pre-v25` → `3d1096f2`：v25 数据库迁移前备份。
- `Database::init` 在 `version > 0 && version < SCHEMA_VERSION` 时自动生成 pre-migration 备份文件。

---

## 3. 已锁定的决策

| 编号 | 决策 | 落地形态 |
|---|---|---|
| A′ | 全局单激活，一个上游服务四个入口 | `providers.is_current` 上建唯一索引；DAO 统一走 `ProviderService::active_provider(db, app)` |
| B′ | 只提供协议，不再按客户端配置 / 托管客户端 | 删 `live/`、`mode/` 与 13 个 `*_config.rs` 写盘路径；增删改切换只改 DB 与激活指针 |
| C′ | 故障转移仅允许同协议 | **待 2c 在 forwarder 落地校验** |
| D′② | `proxy_config` = 代理行为参数表，按入口协议分四行 | 四行保留 `app_type` 主键，新增 `protocol` 列标记：`anthropic←claude` / `openai_responses←codex` / `openai_chat←grokbuild` / `gemini←gemini`，行为参数零变化 |
| — | Gemini 不新增转换 | 矩阵空白格返回 501 + 可读文案 |

---

## 4. 已完成里程碑要点

**P1 / 2a / 2b-1 / 2e 系列**：从外到内摘除 MCP / Skills；引入协议模型与矩阵（不改行为、随时可停）；
四级证据链 `meta.apiFormat` → codex TOML `wire_api` → base URL 后缀 → 家族兜底，纯 origin 不推断；
切换 / 新增 / 编辑 / 删除全部收敛到 DB 与全局激活指针，不再写任何客户端文件。

**2e-3 + 2d（`3d1096f2`）**：删除 `live/`、`mode/`、全部 `*_direct` / `*_editor`、`live.rs` 及 4 个孤儿文件；
`patch/`、`floor/`、`model_label/` 上提为顶层模块，`editor.rs` 收敛为纯 DB 载荷；
命令层去掉接管 / 模式 / Stack 命令、启动刷新、退出清理与代理 resync；补修 macOS 分支的 `sha256_hex` 引用。

**2b-2（`851f7c58`）**：`providers` PK 收敛为 `id`，新增 `protocol` / `protocol_source` / `auth_style` /
`base_url` / `api_key` / `default_model`；v24→v25 迁移逐行用 `infer::of_provider` 推断、id 去重
（同上游合并取最强证据 / 不同上游加 `-2` 后缀）、`providers_legacy` 留档 + `migration_report`、选全局激活项；
`proxy_config` 四行按 ② 打上入口协议标签。

> **三处有意的方案偏差**（详见 `caswitch-P2-迁移方案-v2.md` §10）：保留 `app_type` 兼容列、
> 沿用 `is_current` 列名、`proxy_config` 保留 `app_type` 主键而只新增 `protocol` 列。
> 原因：历史迁移 `migrate_v13_to_v14` 会把 `proxy_config` 整表重建回 app_type 结构，
> 若按原方案把 `protocol` 设为主键，v13 之前的旧库升级路径会断。228 处 DAO 调用点签名零改动。

---

## 5. 验证基线

| 项 | 结果 |
|---|---|
| `cargo check --all-targets` | **0 error** |
| `cargo test --lib` | **3668 passed / 0 failed / 6 ignored**（`--test-threads=2`，26s） |
| 基线偶发项 | `proxy::hyper_client::tests::bytes_with_limit_*` 本轮通过，仍视为偶发 |
| 新增专项测试 | `database::schema::tests::migrate_v24_to_v25_collapses_namespaces_and_tags_protocols` **ok**（PK 收敛、去重改名、全局单激活、四协议映射、留档与报告） |
| `database::backup` | **34 passed / 0 failed**（本轮修复导入回归后） |
| 前端 `pnpm build:renderer` | **通过** |
| 手工四入口连通 | **未做**（需真实上游） |

### 沙箱跑测试的配方（关键）

默认 `cargo test --lib` 会在链接测试二进制时被 SIGKILL（OOM）。关掉 test profile 的
debuginfo 后即可正常编译运行：

```bash
CARGO_HOME=/tmp/cargo-home RUSTUP_TOOLCHAIN=1.92.0 \
CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_INCREMENTAL=false \
RUSTFLAGS="-C debuginfo=0" \
cargo test --lib --offline database::backup -- --test-threads=1
```

### 本轮修复的两个导入回归（已用测试验证）

1. `Database::memory()` 建的是最新 schema 却不写 `user_version`，导出的 SQL 带
   `user_version=0`，导入时被当成旧库重跑全部迁移，v25 收敛会把供应商再折一次
   （`providers` 变空）。修复：建表后写入 `SCHEMA_VERSION`。
2. 「精简版清空历史供应商」原先写在 v26 schema 迁移里，因此**每次导入备份**都会把
   刚恢复的供应商删掉。已改为纯版本占位，清空只在启动阶段按 `slim_provider_reset_v1`
   执行一次。修复后 `database::backup` 从 25/9 变为 34/0。

---

## 6. 待办工作（按执行顺序）

### 6.1 2c — forwarder 接矩阵（下一步，改动最小、可独立验证）

现状实测：`Matrix::plan` 定义在 `src-tauri/src/protocol.rs:217`，但**目前只在 `protocol.rs` 的单元测试里被调用**，
`src-tauri/src/proxy/` 下没有任何生产代码引用它。

1. `proxy/handler_context.rs` / `proxy/provider_router.rs` 确定入口协议，映射关系：
   `claude→Anthropic`、`codex→OpenAiResponses`、`grokbuild→OpenAiChat`、`gemini→Gemini`
2. `proxy/forwarder.rs` 读激活供应商的 `protocol` 列，调 `Matrix::plan(inbound, upstream)`
3. `Passthrough` 直转；`Convert` 走已有转换器；`Unsupported` 返回 **501** + 中文可读文案
   （实际文案由 `Matrix::unsupported_message` 生成："当前入口使用 … 协议，激活的供应商是 … 协议，
   两者之间没有可用的转换器。请切换供应商，或改用与该供应商协议匹配的入口。"）
4. 钉死矩阵断言：**4 透传 + 7 转换 + 5 不支持**（`protocol.rs` 已有覆盖测试，确认口径一致；
   `Chat→Anthropic` 在 2026-11-06 补齐，见 §12）
5. 顺带落实 C′：故障转移换供应商时校验协议相同
6. **验收**：四条入口 × 对应上游各手工连通一次；5 个空白格全部 501 且文案可读
   （2026-11-06：9 格已用真实上游连通，5 个 501 格已自动化验证，见 §12）

### 6.1 2c 完成记录（2026-11-05）

- `forwarder.rs`：新增 `inbound_protocol_for`（按路径判定入口协议：`/chat/completions`、
  `/responses`、`/models/…`、`/messages`），`provider_protocol` 复用 `infer::of_provider`；
  `forward_with_retry_inner` 逐家调用 `Matrix::plan`，`Unsupported` 直接返回 501。
- C′：`same_protocol_failover_chain` 只保留与链头同协议的故障转移供应商，跨协议跳过并打 `FO-006`。
- 501 文案统一由 `Matrix::unsupported_message` 生成（`ProtocolUnsupported` → HTTP 501）。
- 单测：`failover_chain_stops_at_a_different_protocol` 等 4 条。

### 6.2 2f — 前端收尾清理（原「四入口」方案已被 §11 的单 API 入口取代）

- 前端改造清单见 `caswitch-精简重构规划.md` §六；四条 base URL + 配置片段**只展示不写文件**，配复制按钮
- 清理大删除后残留的 **约 241 处 `dead_code` 警告**（`codex_config.rs` 占 79 处，全是 Stack / 官方模型目录 / CLI 候选符号）
- 删 5 张已下线的表：`mcp_servers`、`skills`、`skill_repos`、`prompts`、`proxy_live_backup`
  （**v25 迁移里特意推迟到此处**：备份 / 恢复与并发测试仍读写它们，现在删会破坏回归覆盖）
- `pnpm build` 全量验证
- **验收**：`git grep AppType` 归零；命令数 380 → ~200；四入口前端可配可切

> **本轮实测结论（暂缓 DDL）**：`prompts` 仍被 `services/prompt.rs`、`profile`、`sync_protocol`、
> deeplink 使用；`proxy_live_backup` 虽无生产调用，但 `database/backup.rs` 有 6 处测试直接读写。
> 直接 `DROP TABLE` 会让备份/恢复与迁移回归静默失效，且在沙箱内无法跑 `cargo test --lib` 验证。
> 因此 2f 的 DDL 与 dead_code 清理（175 条，集中在 `session_manager` 与 `codex_config`，
> 两者仍被用量统计与订阅路径引用）单列为后续任务，本轮只完成前端构建验证。

### 6.3 P3 — 强制默认模型（已完成）

- 前端唯一「默认模型」仍落 `settings_config.env.ANTHROPIC_MODEL`，同时写入 `providers.default_model` 列。
- 三种 body 协议沿用 `apply_model_mapping`；Gemini 入站新增 URI 改写
  `rewrite_gemini_endpoint_model`（`/models/{model}:action` 只换模型段）。
- 单测覆盖 URI 改写与空值透传。

### 6.4 P4 — Chat→Responses 转换链路（已完成）

- 新增 `transform_chat_responses.rs`（`chat_request_to_responses`）与
  `streaming_responses_chat.rs`（`responses_response_to_chat` + `create_chat_sse_stream_from_responses`）。
- `Matrix::plan` 的 `OpenAiChat → OpenAiResponses` 由 Unsupported 改为 Convert。
- `forwarder` 改 endpoint（`/chat/completions`→`/responses`，含完整 base URL 形态）与 body；
  `handle_chat_completions` 按供应商协议把 JSON / SSE 响应转回 Chat。
- 覆盖文本、图片、工具调用、推理摘要、usage、错误事件。
- 2026-11-06 追加：`Chat→Anthropic` 直连转换（`transform_chat_anthropic.rs`）补齐，矩阵变为
  7 转换 / 5 不支持；两格均通过真实上游验证，见 §12。

### 6.5 P5 — 改名与收尾（改名已完成）

**命名口径（用户 2026-11-05 确认）**：统一用 `ca-switch`。

已完成：

| 项 | 落地 |
|---|---|
| 包名 | `package.json` → `ca-switch` |
| crate | `Cargo.toml` → `ca-switch` / `ca_switch_lib`，`main.rs` 同步；集成测试 `use ca_switch_lib` |
| 打包 | `tauri.conf.json` → `productName: CA Switch`、`identifier: com.ca-switch.desktop` |
| 数据目录 | 默认 `~/.ca-switch/`、数据库 `ca-switch.db`、日志 `ca-switch`；`migrate_legacy_config_dir` 在新目录无库时从 `~/.cc-switch` 复制旧库（Windows `HOME` 兜底同样走复制） |
| 深度链接 | `.desktop` 文件名改为 `ca-switch-handler.desktop` |
| 前端品牌 | About / 数据库升级文案改为 CA Switch |

- 文档资产：删除 `assets/partners`(13M)、`assets/release-notes`(4.9M)、`assets/screenshots`、
  `docs/images`(11M)、`docs/release-notes`、`docs/user-manual`、`docs/guides`、
  `README_{JA,DE,ZH}.md`、`CHANGELOG.md`；`README.md` 重写为 CA Switch 说明。
- 首启向导：`FirstRunNoticeDialog` 增加「四条入口地址」区块（读取实际监听端口，
  逐条复制，只展示不写文件），文案改为 CA Switch 工作方式。

- i18n 保守裁剪：脚本扫描确认整段命名空间在 `src` 中零引用后，从 5 个语言包删除
  `sessionManager`(254)、`mcpPage`(159)、`skillsPage`(138)、`appsPage`(37)、`appMatrix`(22)、
  `proxyConfig`(14)、`claudeCode`(2)、`appSwitcher`(1)；键数 4,480 → 3,853。
  其余命名空间只需逐键裁剪，留待后续。

---

## 7. 技术债与已知风险

| 项 | 说明 | 处置 |
|---|---|---|
| 175 条 `dead_code` 警告 | 大删除后残留，不阻塞编译 | **已闭环**：最终 0 error / 0 warning，见 §11 |
| 基线偶发失败 | `hyper_client::bytes_with_limit_*` | 本轮全绿，保留为已知偶发 |
| `app_type` / `is_current` 兼容列 | v25 有意保留，DAO 用 `?N = ?N` 恒真条件保持签名不变 | 2f 后统一收敛 |
| 5 张已下线表 | v25 未删 | **已闭环**：schema v28 迁移 DROP，见 §11 |
| 前端构建 | 已通过 `typecheck` + `build:renderer` | 已闭环 |
| 命名口径 | 已确认统一为 `ca-switch`（分支 / 包 / crate / 目录 / identifier） | 已闭环 |
| 远程分支策略 | `origin/ca-switch` → `2ea72fce`（当前 HEAD，工作区干净）；`origin/fork/build` 未改动 | 已闭环 |

### 推送记录（2026-11-05，已完成）

**结果**：`origin/ca-switch` 已创建并指向 `609f0d9c`，本地 `origin/ca-switch` 已同步，
工作区干净、待推送提交数为 0；`origin/fork/build` 全程未改动。共推送 14 个提交
（P1 → 2b-2 的 11 个 + 3 个文档提交）。此后又推送 20 个提交至 `2ea72fce`（见 §13）。

排查中踩过的坑，留作凭据配置参考：

- 系统 GitHub 连接器的 App 用户令牌（`ghu_`）：git HTTPS 写返回
  `Permission to forlzc/cc-switch.git denied to forlzc`（403）；REST 写探测为
  `Resource not accessible by integration` → 不适用于本仓库 git 写操作。
- 第一枚 fine-grained PAT（`github_pat_`、93 字符）：REST `Bearer` / `token` / Basic
  均 `401 Bad credentials`，git 端 `Invalid username or token` → **令牌本身无效**。
- 第二枚 fine-grained PAT 初次校验：REST `/user` 200（有效），但 git 写 403；
  可逆的 REST 写探测 `403 Resource not accessible by personal access token`
  → **令牌有效但授权范围不含该仓库写权限**。用户更新 PAT 权限后，写探测变为
  `CREATE_REF=201` / `DELETE_REF=204`，git 推送随即成功。
- 间歇性 `github.com:443` 连接超时（`api.github.com` 始终可用），但多次连接成功并返回
  明确 401/403，可排除纯网络原因。

> ⚠️ **判据陷阱**：REST `GET /repos/...` 的 `permissions.push = true` 反映的是**账号**
> 对该仓库的权限（owner 全为 true），**不是** fine-grained PAT 被授予的范围。
> 判断 PAT 能否 push，必须看 git 端点或 REST 写操作（`POST /git/refs`）的返回码。

**凭据要求（已验证生效，后续重新生成照此办理）**：

1. **classic PAT（`ghp_`）勾 `repo` scope** —— 最稳，覆盖所有仓库（含 fork），
   不受 fine-grained 的仓库选择限制；
2. fine-grained PAT：Resource owner `forlzc`、Repository access 选 **All repositories**
   （`cc-switch` 是 fork，常不在「Only select repositories」列表里）、
   Repository permissions → **Contents: Read and Write**、确认已生效并完整复制；
3. 连接器路线需确保 GitHub App 授权清单包含 `cc-switch` 与 Contents 写权限，
   否则得到 `Resource not accessible by integration`。

凭据均用临时环境变量注入（`git push <带凭据 URL> ca-switch:refs/heads/ca-switch`），
不写入仓库配置、不回显；`branch.ca-switch.merge` 已改指 `refs/heads/ca-switch`，
避免无参数 `git push` 误推到 `origin/fork/build`。

> 🔒 已明文出现在对话中的两枚 PAT 建议立即在 GitHub 设置里撤销：GitHub 会扫描泄露的
> 密钥并可能自动令其失效，明文令牌也不宜再次使用。

---

## 8. 常用命令

```bash
cd /workspace/cc-switch
git status
cd src-tauri && cargo check --all-targets && cargo test --lib
cd .. && pnpm build          # 2f 时
```

## 9. 相关文档

- `caswitch-精简重构规划.md` — 总规划、P0–P5 分解、协议矩阵、前端清单、数据迁移
- `caswitch-P0-基线.md`、`caswitch-P1-完成报告.md`
- `caswitch-P2-迁移方案.md`（v1）、`caswitch-P2-迁移方案-v2.md`（§9 状态表、§10 2b-2 落地备注）

---

## 10. 通用代理去客户端化（2026-11-05 追加）

### 10.1 用量统计只保留代理流量

- 删除会话日志用量后端链路：`services/session_usage*`、各 agent 的 `session_usage_*`、
  `session_manager/**`、`kimi_session.rs`；`commands/usage.rs` 去掉同步 / 重建 / 数据源命令；
  `lib.rs` 去掉后台定时同步与相关命令注册；`settings.rs` 去掉 `session_auto_sync_enabled`。
- 前端：`useUsageAutoRefresh` 只保留滚动范围的每分钟重查；用量页去掉同步按钮、会话扫描开关、
  Codex 重建与「上次同步」；`UsageSettingsSheet` 只剩路由日志入口、显示与定价；
  去掉应用筛选 chips、请求日志的应用图标列、估算速度/首字（会话日志专属口径）。
- schema **v27**：删除 `proxy_request_logs` 中非 `data_source='proxy'` 的历史行，
  DROP `session_log_sync` / `session_usage_dedup`；新库不再创建这两张表。
- `usage_stats.rs` 的供应商名回退改为 `COALESCE(name, provider_id)`，去掉各 agent 的会话占位名。

### 10.2 全软件去客户端名称

- 导航只保留一个入口，名称改为「供应商」；`AppGlyph` 改为通用服务器图标，不再渲染各客户端品牌图标。
- `APP_DISPLAY_NAME` 改为协议名：Anthropic / OpenAI Chat / OpenAI Responses / Gemini；
  供应商卡片、页头、定价面板、托盘统一走这套名称。
- 设置页下线「应用配置」分组（各客户端目录 / Claude 插件 / Codex 认证），文件删除；
  App 页「配置目录」菜单项移除。
- 5 个语言包把文案中的 Claude / Codex / Gemini CLI / Grok / Kimi / OpenCode / OpenClaw /
  Hermes / Antigravity / MiniMax / Pi / DSH 统一替换为 Anthropic / OpenAI / Gemini 协议名；
  `app.description` 改为「通用 API 代理与用量统计工具」。

### 10.3 验证

- `cargo check --all-targets`：0 error。
- `pnpm typecheck`：通过。
- 用量相关前端测试（RequestLogTable / ProviderStatsTable / UsageDashboard / UsagePage /
  useUsageAutoRefresh / usageFormat）：全部通过。

### 10.4 后续（结构性收敛，未完成）

- `src-tauri` 仍有 13 个 `*_config.rs` 与 14 个 `AppType` 变体被 provider / deeplink /
  prompt 路径引用；彻底删除需要按协议重写 provider 表单的保存契约与 deeplink 导入。

### 10.5 前端去 agent 品牌（2026-11-06）

用户口径：软件是通用代理，**整个软件**都不出现具体 agent，不只是用量统计。

已完成：

- **供应商页**：`ProviderCard` 重写为通用卡（图标 / 名称 / 地址 / 健康 / 用量脚本），
  删除 Copilot / Codex OAuth / xAI / Agy / Kimi / Claude 账号额度、官方订阅额度、终端入口；
  `ProviderList` 只保留切换态列表，删除 opencode / openclaw / hermes / pi / dsh / omo 分支、
  DSH 卡片与终端回调；空状态不再提供「导入当前配置」（通用代理无客户端配置可导入）。
- **供应商表单**：`ClaudeFormFields` 删除全部 OAuth 区块与预设模板参数；`ProviderForm` 收敛为
  单一 Anthropic 协议表单（名称 / 地址 / Key / 模型 / 协议格式 / 请求覆盖 / JSON），
  删除其余 10 余个 appId 分支；`CustomUserAgentField` 去掉客户端 UA 预设，只留自由输入。
- **删除的组件/配置**：Kimi/Mcode/Pi/OpenClaw/Hermes/OpenCode/Omo/ClaudeDesktop/GrokBuild/
  Codex/Gemini 表单、Codex/Gemini 配置编辑器、OAuth 区块与账号管理面板、各 agent 额度页脚、
  `ProviderPresetSelector`/`presetGroups`/`presetStep`、通用供应商面板、settings/auth 账号组；
  对应单测与死 hook（useManagedAuth / useCodexOauth / useXaiOauth / useKimiOAuth / useCopilotAuth 等）一并删除。
- **其它可达 UI**：设置页去掉「首选终端」；`UsageScriptModal` 去掉 Copilot 与官方订阅模板；
  `routingReason` 只按协议给原因；README / package.json / 托盘入口名改为协议名；
  i18n 中 `CC Switch` → `CA Switch`，删除 14 个无引用的顶层命名空间（含 authCenter / prompts /
  claudeDesktop / mcode / copilot 等）。
- **项目（Profile）文案**：`profiles.switcherTooltip.*` 与 `profiles.createDescription.*`
  从「供应商 / MCP / Skills / 记忆文件」改为「供应商配置」，5 个语言包同步。
- **验证**：`pnpm typecheck` 通过；`pnpm vitest run tests/components tests/integration`
  67 文件 / 443 用例通过；`pnpm build:renderer` 通过。

上述两项已在 §11 完成：

- 预设数据已删除，`AppId` 已收敛为单一 `"claude"`（对外显示「API」）。
- 后端 `AppType` 已收敛为单变体，`*_config.rs`、deeplink、托盘中的其他 agent 文案已清理。

---

## 11. 最终收尾（2026-11-06）

### 11.1 最终产品口径

- 软件只保留**一个入口**，显示名「API」，协议由供应商配置里的「上游格式」下拉选择：
  `anthropic` / `openai_chat` / `openai_responses` / `gemini_native`。
- 不出现任何具体 agent/客户端功能或品牌；图标统一使用通用服务器图标，Claude 图标已删除。
- 供应商配置保留：Key、地址、完整 URL、认证字段、上游协议、思考档位、UA/请求覆盖，以及
  从服务商 `/models` 拉取后**直选一个模型**（不再有多档模型映射）。

### 11.2 前端完成项

- `AppId` 收敛为 `"claude"`，`APP_IDS` 只有一项，显示名统一为「API」。
- 删除 Claude 插件联动、onboarding、配置目录、live 直写、官方账号额度、编辑器投影、
  MCP/Skills/记忆槽位、Claude Code 模型映射与 Stack/聚合发布。
- 删除 `BrandIcons.tsx`、`types/omo.ts`、`tomlUtils.ts`、`skillErrorParser.ts` 等无引用模块。
- 删除接管/路由目标/恢复式停止/默认导入等指向不存在后端命令的死封装；
  `useModeActions`、路由切换提示及测试一并删除。
- i18n：删除 `claudeConfig`、`providerPreset` 等整段命名空间与 100+ 条 Claude 历史键，
  以及后续无引用的 `mcp`、`skills`、`omo`；`apps` 只保留 `claude: "API"`。

### 11.3 后端完成项

- `AppType` 只剩 `Claude`（`"claude"`，兼容 `"anthropic"`）；13 个其他 app 的 `*_config.rs`、
  命令、服务、OAuth/托管账号、订阅额度、托盘/深链/数据库种子全部删除。
- 删除 Prompt/CLAUDE.md 子系统：`prompt.rs`、`prompt_files.rs`、`services/prompt.rs`、
  `deeplink/prompt.rs`、`dao/prompts.rs` 及 profile/sync/import_export 引用。
- 删除 Claude Desktop 残留（`claude_desktop_mode`、`claude_desktop_model_routes`）、
  Claude/Codex OAuth 用量缓存、编辑器投影命令 `get_provider_editor_view`。
- `dead_code` / 未使用变量警告从 64 条降至 **0**，未使用 `#[allow(dead_code)]`。
- schema **v28**：新增 `migrate_v27_to_v28`，逐表 `DROP TABLE IF EXISTS` 删除
  `mcp_servers`、`skills`、`skill_repos`、`prompts`、`proxy_live_backup`；
  新建数据库不再创建这些表，备份/同步清单同步移除。
- 保留通用代理核心：协议转换、路由、故障转移、用量统计、余额/套餐、模型拉取、测速、
  自定义端点、供应商快照、备份/WebDAV/S3、环境检测、自动启动、托盘基础功能。

### 11.4 验证基线

| 项 | 结果 |
|---|---|
| `cargo +1.94.1 check` | **0 error / 0 warning** |
| `cargo +1.94.1 test --lib` | **1658 passed / 0 failed / 7 ignored**（2026-11-06 最新，含心跳与协议验证用例） |
| `cargo +1.94.1 test --tests` | **3 passed / 0 failed** |
| `pnpm typecheck` | **通过** |
| `pnpm vitest run` | **120 文件 / 787 用例通过** |
| `pnpm build:renderer` | **通过** |

### 11.5 仍未做（非代码完成项）

- 真实上游连通验证：2026-11-06 已完成 9 格（Anthropic / Chat / Responses 三种上游 × 三种入口），
  剩余 2 格需要 Gemini 上游 Key（`Anthropic→Gemini` 转换、`Gemini→Gemini` 直通），见 §12。
- ja / tr / zh-TW 语言包存在历史翻译缺口（键数少于 zh/en），与本次删除无关，不影响运行。
- 少量历史迁移函数保留旧表重建逻辑，已用 `table_exists` 守卫；旧库升级到 v28 后五表会被删除。

---

## 12. 协议矩阵补齐与真实上游验证（2026-11-06，待提交）

### 12.1 本轮目标

原 CC Switch 没有 `Chat 入口 × Anthropic 上游` 这一格，`Chat 入口 × Responses 上游` 也只在
P4 之后可用。本轮把两格都做成直连转换，并用真实上游逐格验证「入口协议 → 上游协议 → 响应形状」。

### 12.2 最终矩阵（16 格）

| 入口 ＼ 上游 | Anthropic | OpenAI Chat | OpenAI Responses | Gemini |
|---|---|---|---|---|
| Anthropic | 直通 | 转换 | 转换 | 转换 |
| OpenAI Chat | **转换（本轮）** | 直通 | 转换（P4） | 501 |
| OpenAI Responses | 转换 | 转换 | 直通 | 501 |
| Gemini | 501 | 501 | 501 | 直通 |

口径：**4 透传 + 7 转换 + 5 不支持**，与 `protocol.rs` 的 `matrix_covers_exactly_seven_cells` 一致。

### 12.3 新增/修改文件

| 文件 | 内容 |
|---|---|
| `providers/transform_chat_anthropic.rs`（新增） | Chat ⇄ Anthropic 直连：system / 图片 / 工具定义 / `tool_use`+`tool_result` 历史 / 合并同角色消息 / SSE / usage / 错误体 |
| `providers/transform_chat_responses.rs`（P4 已有） | Chat → Responses 请求方向 |
| `providers/streaming_responses_chat.rs`（P4 已有） | Responses → Chat JSON / SSE |
| `protocol.rs` | 矩阵补 `(OpenAiChat, Anthropic)`，注释与测试同步 |
| `proxy/forwarder.rs` | 新增 `chat_to_anthropic` 判定：endpoint、body、Accept、指纹头、identity 编码 |
| `proxy/handlers.rs` | `handle_chat_completions` 按供应商协议分派，Chat←Anthropic 响应转换 |
| `proxy/error_mapper.rs` | `ProtocolUnsupported → 501` |
| `proxy/server.rs` | 三条真实/半真实验证用例（见 §12.4） |

### 12.4 验证用例

| 用例 | 覆盖 | 结果 |
|---|---|---|
| `real_upstream_protocol_matrix`（`#[ignore]`，需真 Key） | 3 入口 × 3 上游 × 流式/非流式，共 18 个请求；校验状态码 **和** 响应/SSE 形状 | 18/18 通过 |
| `real_upstream_chat_client_responses_provider`（`#[ignore]`） | Chat×Responses 工具调用、多轮 `tool` 回传、流式工具分块与 usage | 通过 |
| `unsupported_protocol_cells_return_501_with_readable_message`（常规用例，无网络） | 按 `Matrix::unsupported_cells()` 逐格断言 501 + 中文文案 | 5/5 通过 |

运行方式：

```bash
cd src-tauri
E2E_UPSTREAM_API_KEY=... E2E_UPSTREAM_BASE_URL=https://bailucode.com/openapi/v1 \
E2E_UPSTREAM_MODEL=bailu-2.7 E2E_UPSTREAM_PROXY=http://127.0.0.1:18080 \
cargo +1.94.1 test --lib real_upstream -- --ignored --nocapture
```

实测结论：9 格（Anthropic / Chat / Responses 上游 × 三种入口）全部 200 且响应形状与入口协议一致；
新增的 `Chat×Anthropic` 与 `Chat×Responses` 工具调用均正常。Gemini 两格（`Anthropic→Gemini`、
`Gemini→Gemini`）缺 Gemini Key，尚未真实验证。

### 12.5 基线 tag 补齐

文档原记载的 `caswitch-base`（重构前基线）此前不存在，现已在 `bc4c4c45`
（`Merge branch 'farion1231:main' into fork/build`，即 `cee21206` 之前）补打：

```bash
git tag -a caswitch-base bc4c4c45 -m "caswitch 重构前基线（fork/build 合并点，cee21206 之前）"
```

### 12.6 状态

本轮改动已全部提交并推送：`b6b5d195`（协议矩阵）→ `09835778`（Windows 构建）→ `ad576f56`、`a684854a`（修复）→
`2ea72fce`（更名与链接同步）。远端 `origin/ca-switch` 与本地一致，均指向 `2ea72fce`；
`v4.0.25` tag 指向同一提交并已发布，详见 §13。

---

## 13. 最新修复与 v4.0.25 发布（2026-11-06）

### 13.1 本轮修复

| 问题 | 修法 | 提交 |
|---|---|---|
| Chat×Responses 长推理静默被判断流（`Response stream ended without a finish reason`） | 转换流改 `tokio::select!`，静默 15s 补空 delta 心跳；新增暂停时钟单测 | `ad576f56` |
| Anthropic 入口模型名被透传，客户端选项未替换为配置模型 | 转发前用供应商配置的上游模型覆盖请求体 `model`，按协议读 `ANTHROPIC_MODEL` / `OPENAI_MODEL` / `GEMINI_MODEL`，空值不覆盖 | `a684854a` |
| 配置 JSON 固定 Anthropic 字段，与「上游格式」无关 | 新增 `claudeApiFormatConfig.ts`；表单地址 / Key / 模型按协议读写，切协议时自动迁移旧键；后端 Base URL 提取同步 | `a684854a` |
| 上游格式文案仍写「原生 / 需转换」 | 改为纯协议名，5 个语言包同步 | `a684854a` |
| 用量统计「应用」列恒为 Anthropic | 删除列、详情行与映射，默认列 10 → 9 | `a684854a` |
| UA 预设下拉与模型列表刷新回归 | 恢复分组预设（含 Claude Code 2.1.318 / Codex 0.167.0 / Kilo-Code 7.8.27）；获取按钮常驻可重复刷新 | `ad576f56` |
| 仓库更名后代码内链接仍旧指向 `cc-switch` | README、关于页、更新端点、深链、Cargo 元数据、文档统一改为 `ca-switch`；便携包资产名补 `v` | `2ea72fce` |

### 13.2 发布

- 版本 `4.0.25`，tag `v4.0.25` → `2ea72fce`，Release 由该提交重新构建并发布：
  https://github.com/forlzc/ca-switch/releases/tag/v4.0.25
- 资产：`CA.Switch_4.0.25_x64_en-US.msi`（12,599,296）、
  `CC-Switch-v4.0.25-Windows-Portable.zip`（12,366,805）、`CA.Switch_4.0.25_amd64.deb`（12,857,090）
- Windows 由 GitHub Actions `windows-2022` 原生构建；Linux deb 为本机构建，均关闭 updater 产物（无签名私钥）。
- 旧 `fork-v1/2/3` Release 与 tag 已清理；本地 `caswitch-base` 基线 tag 保留。
- 仓库已由 `cc-switch` 更名为 `ca-switch`，旧地址由 GitHub 自动重定向；本地 `origin` 已更新。

### 13.3 已知未决

- `response.failed` / SSE `error` 仍不补 `finish_reason` 与 `[DONE]`，客户端会把上游真实原因显示为
  「没有 finish reason」；建议后续改为错误分块 + 终止分块。
- 首包前（未收到 `response.created`）的静默仍无心跳，超长 TTFT 上游仍可能触发超时。
- `Gemini` 两格真实上游验证仍缺 Key（`Anthropic→Gemini`、`Gemini→Gemini`）。

