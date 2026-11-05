# caswitch 精简重构：进度与待办

> 更新时间：2026-11-05 16:49（GMT+8）
> 仓库：`/workspace/cc-switch`　远端：`https://github.com/forlzc/cc-switch.git`
> 分支：`caswitch` → **`ca-switch`**（已推送 `origin/ca-switch` @ `609f0d9c`）
> 基线：tag `caswitch-base` / `origin/fork/build` (`bc4c4c45`)，本地领先 11 个提交
> 工作区：干净

---

## 1. 一句话现状

P0–P2 的 2a、2b、2e、2d、2b-2 与 **2c** 已落地；**P3、P4 完成**；**P5 改名完成**。
**2f 只剩两件事**：删除 5 张下线表（需同步调整 prompt/live-backup 路径与备份测试）、
清理约 175 条 `dead_code` 警告。当前后端 `cargo check --all-targets` 0 error，
前端 `pnpm typecheck` 与 `pnpm build:renderer` 均通过。

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
   （例："当前供应商为 Gemini 协议，OpenAI Chat 入口不支持，请改用 Claude Code 入口或切换供应商"）
4. 钉死矩阵断言：**4 透传 + 6 转换 + 6 不支持**（`protocol.rs` 已有覆盖测试，确认口径一致）
5. 顺带落实 C′：故障转移换供应商时校验协议相同
6. **验收**：四条入口 × 对应上游各手工连通一次；7 个空白格全部 501 且文案可读

### 6.1 2c 完成记录（2026-11-05）

- `forwarder.rs`：新增 `inbound_protocol_for`（按路径判定入口协议：`/chat/completions`、
  `/responses`、`/models/…`、`/messages`），`provider_protocol` 复用 `infer::of_provider`；
  `forward_with_retry_inner` 逐家调用 `Matrix::plan`，`Unsupported` 直接返回 501。
- C′：`same_protocol_failover_chain` 只保留与链头同协议的故障转移供应商，跨协议跳过并打 `FO-006`。
- 501 文案统一由 `Matrix::unsupported_message` 生成（`ProtocolUnsupported` → HTTP 501）。
- 单测：`failover_chain_stops_at_a_different_protocol` 等 4 条。

### 6.2 2f — 前端四入口 + 收尾清理

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
| 175 条 `dead_code` 警告 | 大删除后残留，不阻塞编译 | 2f 清理（`session_manager`、`codex_config` 仍被用量/订阅引用，需逐项删） |
| 基线偶发失败 | `hyper_client::bytes_with_limit_*` | 本轮全绿，保留为已知偶发 |
| `app_type` / `is_current` 兼容列 | v25 有意保留，DAO 用 `?N = ?N` 恒真条件保持签名不变 | 2f 后统一收敛 |
| 5 张已下线表 | v25 未删 | 2f 删 |
| 前端构建 | 已通过 `typecheck` + `build:renderer` | 已闭环 |
| 命名口径 | 已确认统一为 `ca-switch`（分支 / 包 / crate / 目录 / identifier） | 已闭环 |
| 远程分支策略 | 已推送 `origin/ca-switch` @ `609f0d9c`（14 个提交）；`origin/fork/build` 未改动 | 见下方「推送记录」 |

### 推送记录（2026-11-05，已完成）

**结果**：`origin/ca-switch` 已创建并指向 `609f0d9c`，本地 `origin/ca-switch` 已同步，
工作区干净、待推送提交数为 0；`origin/fork/build` 全程未改动。共推送 14 个提交
（P1 → 2b-2 的 11 个 + 3 个文档提交）。

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
