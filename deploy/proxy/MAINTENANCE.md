# 美国节点自动选择与手动订阅更新

这些工具维护独立的 Mihomo 实例。所有订阅、节点凭证、controller secret、实际端点、认证记录及备份由操作者保存在私有目录，仓库仅提供代码和示例。没有订阅刷新 timer。

## 安装与私有配置

需要 Linux、Python 3.10+、PyYAML，以及已安装的 Mihomo。Python 依赖可装进虚拟环境：

```bash
python3 -m venv .venv
.venv/bin/pip install -r scripts/proxy/requirements.txt
```

将 `maintenance.example.json` 复制为私有配置，填写现有实例的信息。配置中的 `source_file`、`subscription_url_file`、`state_dir`、可选 `lock_file`、目标 `directory` 和 `manifest_file` 相对于维护配置文件所在目录解析；目标的 `binary`、`config_file` 和 `selections_file` 相对于目标 `directory` 解析。也可以使用绝对路径。实际实例端口不需要与示例相同。

- `subscription_url_file`：仅包含真实 HTTPS 订阅链接的私有文件；不要把链接放进命令参数或仓库。
- `source_file`：已存在的原始订阅 YAML；更新成功后也会更新这个文件。
- `state_dir`：候选、验证日志和备份的私有目录，工具设为 `0700`，新文件为 `0600`。替换已有配置或选择记录时保留其 uid、gid 和权限位，回滚同样保留，避免破坏实例原有读取权限。
- `download_proxy`：下载订阅使用的本地 HTTP 代理。工具清除该请求的 `NO_PROXY` 例外，避免意外直连。
- `lock_file`：默认 `state_dir/maintenance.lock`。迁移已有维护工具时，让它们共享同一个锁，并停用旧的并发更新入口。父目录必须已存在。
- `node_identity=name-hostname`：节点名应以 `@hostname:port` 结尾，用 hostname 稳定匹配订阅名称变化。普通节点名可用 `server`，但每个 server 必须唯一。
- `targets`：同一订阅的实例列表；每个目标配置都是现有的完整 Mihomo 配置。必须具有本地 `127.0.0.1:port` controller，认证 secret 从该私有 YAML 读取。
- `selections_file`：可选的 JSON 选择记录；已有记录随名称变化迁移，美国自动组应用时写入新的主选择。
- `us_auto`：只在需要美国自动选择的目标上设置。`selector` 是现有静态 `select` 组，`group` 是美国自动组名，`replaced_groups` 是要迁移的旧自动组名。未指定替换的其他组继续保留。

两个命令都需要读取实例配置和访问本地 controller。应用还需要写配置、选择记录及状态目录的权限；不必强制以 root 运行。如果使用 systemd，按实际目录替换 `mihomo.service.example` 的占位符，每个实例安装独立 unit。先用 `systemd-analyze verify` 验证渲染后的 unit，再按已有运维流程启用。模板不安装 timer。

## 认证美国节点

`us-node-verification.example.json` 仅演示结构，示例域名不可连接。不要直接使用示例认证记录。

先逐个通过候选节点实际测量出口国家，例如对比 Cloudflare trace 的 `loc` 与独立 IP 地理来源的 `country`。两个来源一致为 `US` 后，将测量时间、证据位置和认证记录写入私有 manifest。`verified_at_utc` 必须是 UTC 时间。记录键及 `hostname` 使用所选节点身份；固定 `server`、`type`、`port`，保存出口 IP 和证据来源。IP 地址是测量结果，不是节点入口地址。

工具从 manifest 中的美国记录生成静态成员列表。没有认证、认证端点已变化、认证节点已消失或没有美国成员时均拒绝应用。新增节点需要操作者重新测量并更新 manifest；不根据节点名推断国家。

## 创建或维护美国自动组

以下命令从仓库根目录运行，将占位路径换成私有配置路径：

```bash
# 生成候选并用 Mihomo 验证，保持运行配置和选择不变。
.venv/bin/python scripts/proxy/us_auto.py --config /path/to/private/maintenance.json --target business

# 再次生成、验证、备份，然后应用与重载。
.venv/bin/python scripts/proxy/us_auto.py --config /path/to/private/maintenance.json --target business --apply

# 检查已运行的美国自动组、健康状态及实际出口。
.venv/bin/python scripts/proxy/us_auto.py --config /path/to/private/maintenance.json --target business --verify
```

默认参数为 `url-test`、180 秒、30ms 容差、5 秒超时、`lazy=false`，检测 URL 期望返回 204。Mihomo 定期选择组内响应较快的健康节点；30ms 容差用于减少频繁切换，不保证每一刻使用绝对最低延迟节点。检测 URL、间隔、容差、超时与期望状态可以配置。

应用将目标主选择切至美国自动组，其他实例及其选择保持不变。重载后触发组检测，检查成员、选择和未固定状态，并经目标 `egress_proxy` 获取实际 Cloudflare trace，要求 `loc=US`。代理端口必须匹配该实例配置的 `mixed-port` 或 HTTP `port`，不使用其他代理或转发入口。`egress_check_url` 必须返回 Cloudflare trace 格式。

## 手动刷新订阅

```bash
# 下载并验证所有目标的候选配置；不会重载。
.venv/bin/python scripts/proxy/subscription_update.py --config /path/to/private/maintenance.json

# 验证后应用；保留规则、分组及运行中的手动选择。
.venv/bin/python scripts/proxy/subscription_update.py --config /path/to/private/maintenance.json --apply

# 使用已下载的私有 YAML，避免再次请求订阅。
.venv/bin/python scripts/proxy/subscription_update.py --config /path/to/private/maintenance.json --downloaded /path/to/private/downloaded.yaml
```

工具只替换静态节点，迁移节点重命名对应的组成员和选择记录。节点身份列表变化、失效引用、规则直接引用被改名节点、美国节点端点变化以及 Mihomo 验证失败时拒绝应用。含 provider 的订阅需要人工迁移。为所有需要美国认证的组配置 `us_auto`，否则工具无法识别其国家限制。

订阅更新保留已存在的美国自动组。若运行中的主选择是该组，应用后还会检查当前实际出口为美国；主选择是手动节点时保持其选择，不切换去执行自动组出口检查。`--verify` 专门验证主选择已启用美国自动组。

## 回滚与限制

应用前保存原文件与运行中的 `select` 选择，检查验证期间没有配置或交互选择变化。部分写入、重载、选择恢复或出口验证失败时恢复原文件、重载旧配置并恢复选择。备份 `index.json` 记录原路径与对应备份，应用结果记录在私有 `result.json`。返回码：0 成功/预检查通过，1 处理失败，2 候选拒绝，3 应用失败并尝试回滚；检查 JSON 中的 `rolled_back` 或 `rollback_failed`。

若应用期间另有进程编辑了已写入文件，工具拒绝用旧文件覆盖这次编辑，并报告回滚失败，需要操作者核对私有备份。文件替换是逐个原子写入，不是跨文件系统事务，主机断电或强制终止仍需要人工恢复。安排维护窗口，避免同时手动编辑或选节点。

manifest 只固定入口信息，无法识别入口不变而出口换了国家的情况。应用和 `--verify` 检查当前选择的真实出口，Mihomo 日常延迟检测不重新做国家认证。定期重新测量、刷新 manifest；国家证据没有自动过期策略。

## 测试

```bash
.venv/bin/python -m unittest discover -s scripts/proxy/tests -v
```

测试使用合成节点和假 controller，不访问订阅、公共网络或生产服务。真实运行需要兼容 Mihomo 的 controller 分组检测 API；不兼容或返回异常时应用会失败并回滚。
