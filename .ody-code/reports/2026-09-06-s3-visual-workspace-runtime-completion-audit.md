# S3：Visual Workspace Runtime 接入审计

日期：2026-09-06

## 结论

S3 的工程基础已完成，Visual Workspace 不再只是 odyBox 的私有数据模型：Ody app-server 现在拥有版本化协议、原子持久化、修订/补丁/快照记录和变更事件；odyBox 从 Ody 直接生成 TypeScript 契约，并已具有可灰度启用的一条“生成 → Runtime Artifact”链路。

这不等于 S3 的产品退出条件已经达成。默认仍是 legacy，因为当前 odyBox 安装包尚未内置并管理 Ody 可执行文件；在该条件下把 Runtime 设为默认会把缺失二进制变成用户故障，而不是可靠迁移。

## 已交付

- `visualWorkspace/v1` 实验性 app-server API：Project upsert/list、Artifact create/list、Patch apply、PNG Snapshot create、Preview open，以及 `visualWorkspace/changed` 通知。
- Runtime 本地权威存储：`$ODY_HOME/visual-workspace/v1.json`，临时文件 rename 提交、上一版本 `.bak` 恢复、请求幂等键按资源类型隔离。
- 防护：HTML 1 MiB 上限、PNG 12 MiB 上限/签名校验、跨项目拒绝、补丁目标存在性校验、拒绝 `url(...)` CSS 值。
- Runtime 变更会广播给支持该实验 API 的连接；TUI 明确识别为全局非线程事件，不污染会话状态。
- odyBox 已引入由 `ody app-server generate-ts --experimental` 直接生成的协议类型，并提供 `pnpm generate:ody-runtime-types` 更新入口。
- odyBox 使用 `VITE_ODYBOX_VISUAL_RUNTIME=runtime` 灰度开关。开启后，生成链会启动/复用 `ody app-server --stdio`，先写 Runtime Project/Artifact，再写本地 UI 投影；进程不存在、退出或 15 秒超时时回退本地链路。

## 验证

- `cargo nextest run -p ody-app-server-protocol --tests`：231 passed。
- `cargo nextest run -p ody-app-server --lib visual_workspace_processor --no-fail-fast`：2 passed。
- `pnpm exec tsc --noEmit --pretty false`：通过。
- `pnpm exec vitest run src/renderer/services/ody-runtime/visual-workspace-client.test.ts src/renderer/packages/model-calls/toolsets/design.test.ts`：13 passed。

## 尚未满足的退出条件

1. 安装包需携带受版本约束的 Ody 二进制，而不能依赖 PATH 或 `ODYBOX_ODY_RUNTIME_COMMAND`。
2. 在真实用户与自动化场景中比较 legacy/runtime 的生成、补丁、截图和浏览器验证结果、时延、失败率与恢复成功率。
3. Runtime 路由仍需扩展到 Canvas style patch、截图持久化和 Browser 验证；当前已接入生成 Artifact 这一条完整灰度链。
4. 指标达标后，才允许将开关默认设为 `runtime`；保留回退窗口后再删除 legacy 权威写入。
