# Ody Database Tool

Ody 内置数据库查询工具，允许大模型在会话中直接执行 SQL 查询。支持配置多个数据库连接预设，每个项目可独立管理，连接信息保存在 `config.toml` 的 `[services.database]` 下。

## 目录

- [功能概述](#功能概述)
- [支持的引擎](#支持的引擎)
- [配置文件格式](#配置文件格式)
- [TUI 交互配置](#tui-交互配置)
- [LLM 使用 DatabaseQuery](#llm-使用-databasequery)
- [连接 URL 构造规则](#连接-url-构造规则)
- [安全与隐私](#安全与隐私)
- [故障排查](#故障排查)
- [完整示例](#完整示例)

## 功能概述

- 支持 **PostgreSQL**、**MySQL**、**SQLite** 三种数据库引擎。
- 每个项目可保存多个命名连接预设（如 `project_a_postgres`、`local_sqlite`）。
- 可指定一个 `primary` 连接作为默认。
- 大模型调用 `DatabaseQuery` 工具时，默认使用 `primary`；也可通过 `connection` 参数切换到其他连接。
- 使用 `sqlx` 的 `AnyPool` 统一执行 SQL，返回列名与行数据。

## 支持的引擎

| 引擎 | 配置值 | 必填字段 | 可选/特殊说明 |
|------|--------|----------|---------------|
| PostgreSQL | `postgres` | `host`、`port`、`database`、`username` | 可选 `password`、`options.sslmode` |
| MySQL | `mysql` | `host`、`port`、`database`、`username` | 可选 `password`、`options.ssl_mode` |
| SQLite | `sqlite` | `host`（数据库文件路径） | `port`、`database`、`username`、`password` 被忽略 |

> MariaDB 通常兼容 MySQL 协议，可直接使用 `provider = "mysql"`。

## 配置文件格式

数据库配置位于 `config.toml` 的 `[services.database]` 表下。`primary` 为当前默认连接名称，`connections` 为各连接预设。

```toml
[services.database]
primary = "db_connection_1"

[services.database.connections.db_connection_1]
connection = "db_connection_1"   # 必须等于表名 key
provider = "postgres"
host = "127.0.0.1"
port = 5432
database = "project_a"
username = "ranwei"
password = "secret"

[services.database.connections.db_connection_1.options]
sslmode = "disable"
```

新增第二个项目 B 时，只需再加一个连接并切换 `primary`：

```toml
[services.database.connections.project_b_mysql]
connection = "project_b_mysql"
provider = "mysql"
host = "127.0.0.1"
port = 3306
database = "project_b"
username = "root"
password = "another-secret"

[services.database.connections.project_b_mysql.options]
ssl_mode = "disabled"
```

将 `primary` 改为 `"project_b_mysql"` 即可让 LLM 默认连接项目 B。

## TUI 交互配置

在 TUI 聊天界面中，输入 slash 命令：

```
/database
```

会弹出 **Database Connections** 弹窗，顶部有三个 provider 标签页：**PostgreSQL**、**MySQL**、**SQLite**。每个标签页只显示对应引擎的已保存连接。

- 左右方向键：切换 provider 标签页。
- 上下方向键：选择当前标签页中的连接。
- `Enter`：将选中连接设为 `primary`（默认连接）。
- `Tab`：编辑选中的连接。
- `a`：添加一个属于当前标签页 provider 的新连接。
- `d` 或 `Delete`：删除选中的连接，会弹出确认框。
- `Esc`：取消；在删除确认框中按 `Esc` 返回列表。

### 添加 / 编辑连接

进入表单后，需要填写以下字段：

| 字段 | 说明 |
|------|------|
| Name | 连接预设名称，在 `DatabaseQuery` 的 `connection` 参数中使用。 |
| Host / Path | PostgreSQL/MySQL 填主机或 IP；SQLite 填数据库文件路径。 |
| Port | 端口号，SQLite 可忽略。 |
| Database | 数据库/Schema 名，SQLite 可忽略。 |
| Username | 用户名，SQLite 可忽略。 |
| Password | 密码，SQLite 可忽略；输入时会被掩码。 |
| SSL mode | 可选，如 PostgreSQL 的 `sslmode` 或 MySQL 的 `ssl_mode`。 |

Provider 在添加表单中是固定的，由 `/database` 列表中当前选中的标签页决定；编辑时同样不可修改。编辑完字段后，选择 **Save connection** 保存。配置会持久化到 `config.toml`。

> 注意：目前编辑时不能修改已有连接的名称（Name）。如需改名，可删除后重新添加。

## LLM 使用 DatabaseQuery

当 `config.toml` 中配置了至少一个数据库连接并成功加载后，会话中的 LLM 可见 `DatabaseQuery` 工具。

### 工具参数

```json
{
  "query": "SELECT * FROM users LIMIT 5;",
  "connection": "project_b_mysql"   // 可选，默认使用 services.database.primary
}
```

- `query`（必填）：要执行的 SQL 语句。
- `connection`（可选）：命名连接预设。省略时自动使用 `primary`。

### 返回结果

工具返回 JSON：

```json
{
  "columns": ["id", "name"],
  "rows": [
    ["1", "Alice"],
    ["2", "Bob"]
  ],
  "text": "id | name\n----\n1 | Alice\n2 | Bob"
}
```

`text` 字段为可读表格格式，方便 LLM 直接展示；`columns` 与 `rows` 便于程序化处理。

## 连接 URL 构造规则

Ody 在运行时会根据配置自动构造连接 URL：

- PostgreSQL：`postgres://username:password@host:port/database`
- MySQL：`mysql://username:password@host:port/database`
- SQLite：`sqlite://host`（`host` 为数据库文件路径）

密码会经过 URL 编码，因此可包含特殊字符。

## 安全与隐私

- 当前密码以 **明文** 形式保存在 `config.toml` 中。
- 配置文件中的 `Debug` 输出以及 `ody_database` 的日志输出会掩码密码，显示为 `***`。
- 建议将包含数据库密码的 `config.toml` 加入 `.gitignore` 或避免提交到版本库。

## 故障排查

| 现象 | 可能原因 | 处理建议 |
|------|----------|----------|
| LLM 看不到 `DatabaseQuery` 工具 | 配置未加载或没有有效连接 | 检查 `config.toml` 是否有 `[services.database]` 表，并确认至少有一个连接预设。 |
| 调用时报 “Database connection 'xxx' is not configured” | 连接名称不存在或拼写错误 | 核对 `connection` 参数与配置中的预设名称。 |
| 连接失败 | 主机、端口、用户名、密码或数据库名错误 | 用同一份参数在命令行工具（如 `psql`、`mysql`、sqlite 命令）测试连通性。 |
| SQLite 连接失败 | 路径不存在或格式错误 | 确保路径存在且可读写；可尝试绝对路径。 |
| 查询返回 `<unsupported>` | 字段类型不在 `String / i64 / f64 / bool / Option<String>` 范围内 | 避免直接选择复杂类型列，或使用 `CAST` 转换为文本。 |
| 返回空行但 SQL 应返回数据 | 查询语法或权限问题 | 检查日志中的原始错误，或先用 `SELECT 1` 验证连接。 |

## 完整示例

### PostgreSQL

```toml
[services.database]
primary = "local_postgres"

[services.database.connections.local_postgres]
connection = "local_postgres"
provider = "postgres"
host = "127.0.0.1"
port = 5432
database = "ody"
username = "ody_user"
password = "my_password"

[services.database.connections.local_postgres.options]
sslmode = "prefer"
```

### MySQL

```toml
[services.database.connections.local_mysql]
connection = "local_mysql"
provider = "mysql"
host = "127.0.0.1"
port = 3306
database = "ody"
username = "ody_user"
password = "my_password"

[services.database.connections.local_mysql.options]
ssl_mode = "disabled"
```

### SQLite

```toml
[services.database.connections.local_sqlite]
connection = "local_sqlite"
provider = "sqlite"
host = "/Users/ranwei/data/ody.db"
port = 0
database = ""
username = ""
```

切换默认连接时，修改：

```toml
[services.database]
primary = "local_sqlite"
```

