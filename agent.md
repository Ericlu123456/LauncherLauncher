# agent.md

本文件描述**新增/适配一个启动器**的流程，供后续维护者和自动化代理使用。

## 0. 边界与原则

- 只处理用户有权修改的本地文件；不做 DLL/Agent 注入、不联网、不代理流量、不绕过服务端限制。
- **不要在仓库里新建 `launchers/` 目录**，也不要按启动器堆叠未经验证的说明、截图或“已支持版本”承诺。此前的目录因包含不实/易误解内容已移除。
- 文档只写**可复现的事实**：文件哈希、`--inspect` 输出、实际测试结果、失败日志。不要引用未验证的上游提交、内部行为或监管说法。
- 话题敏感时保持中性：只描述本地兼容性维护，不写未经证实的背景或对抗性表述。
- 匹配不到就失败并报告（fail closed），不要猜测地址、扫描特征码或强改不兼容的方法。

## 1. 先判断是否需要改代码

如果只是给某个**已支持格式**（Java JAR / .NET PE）添加一两条规则，不需要动 Rust 代码：

```bash
launcherlauncher --inspect <文件>
launcherlauncher --hack <文件> --dry-run --rules-file rules.json
```

`--rules-file` 的 JSON 规则已经支持 `class` / `method` / `desc` / `action`。只有需要**内置识别**或新的匹配/改写逻辑时，才进入下面的代码流程。

## 2. 架构速览

| 位置 | 职责 |
| --- | --- |
| `src/target.rs` | 识别输入：JAR / PE / 目录；`TargetKind`、`label_for`、版本和 SHA-256 |
| `src/patch/profiles/` | 每个启动器一个规则模块；`mod.rs` 注册并分派 |
| `src/patch/java.rs` | Java `.class` 方法体改写 |
| `src/patch/pe.rs` | .NET PE/CLI 元数据、方法体和 single-file 扫描 |
| `src/launcher.rs` | 备份、应用、恢复、启动 |
| `src/cli.rs` / `src/gui.rs` | 入口和展示 |
| `tests/engines.rs` | 引擎级测试 |

当前只支持：

- `TargetKind::Jar`：ZIP/JAR 里的 Java class；
- `TargetKind::Pe`：托管 PE，或包含托管 PE 的 .NET single-file。

原生 C/C++ PE、NativeAOT、压缩/加密的 single-file 不在当前解析器范围内。不要为了“支持新启动器”而假装支持。

## 3. 适配步骤

### 3.1 收集证据

1. 确认文件类型、版本、SHA-256：

   ```bash
   launcherlauncher --inspect <文件>
   sha256sum <文件>
   ```

2. 在同一版本上做 `--dry-run`，记录命中的规则和跳过原因。
3. 只保留**能复现**的信息；不要把启动器本体、账号数据、个人路径提交到仓库。

### 3.2 识别与标签

在 `src/target.rs` 中：

- 先看现有 `label_for` / `sniff_kind` 是否能覆盖；如果文件名启发式会误判，改成基于元数据/结构的判断。
- 新标签必须同时处理：
  - `label_for` 返回的显示名；
  - `TargetKind`（必须落在现有 Jar/Pe 解析器内）；
  - 版本探测（JAR manifest 或 PE Assembly 表）；
  - 无法识别时的 `Unknown` 分支。
- 识别失败的路径必须明确报错，不要默认猜一个启动器。

### 3.3 添加 profile 模块

在 `src/patch/profiles/` 新建 `<name>.rs`：

```rust
use super::rule;
use crate::patch::{Action, Rule};

pub fn example(remove_updates: bool) -> Vec<Rule> {
    let mut rules = vec![
        rule(
            "example.Feature",
            "isEnabled",
            Some("()Z"),
            Action::ReturnFalse,
            "本地兼容规则：仅对已验证版本生效",
        ),
    ];
    if remove_updates {
        // 仅添加验证过的更新入口
    }
    rules
}
```

然后在 `src/patch/profiles/mod.rs`：

1. `mod <name>;`
2. `pub use <name>::<name>;`
3. 在 `for_launcher(label, ...)` 里增加精确匹配的 `match` 分支。

规则设计：

- 优先使用完整 `class` + `method` + `desc`；通配符只用于确实稳定的命名空间。
- `action` 必须和返回类型兼容；不兼容的规则会跳过，这是预期行为。
- 不要用“名字像某个启动器”作为唯一依据；至少结合文件类型、元数据、版本或结构锚点。
- 不要批量改写泛型/异步/构造器；当前引擎只支持简单方法体替换和现有的专用 loader 改写分支。

### 3.4 测试

至少覆盖：

1. **单元测试**：规则匹配、返回类型兼容、跳过不兼容方法。
2. **引擎测试**：构造最小 JAR/PE fixture，验证改写后的 IL/字节和原始备份。
3. **集成/手工测试**（有合法副本时）：
   - `--inspect` 识别正确；
   - `--dry-run` 只报告、不写盘；
   - 正常写入前生成备份；
   - 启动器能启动；
   - 恢复备份后哈希恢复；
   - 已改写文件再次运行不会重复写。
4. **入口级检查**：规则命中不等于功能可用。上游如果删除了下载入口、按钮或导航项，必须判为不支持；不要用“命中数”冒充“下载功能正常”。
5. **失败路径**：错误文件、截断 PE、缺失元数据、方法体过短，都必须返回错误或跳过，不能 panic。

测试 fixture 应尽量合成/最小化；不要把第三方启动器提交到仓库。

### 3.5 文档与 Issue

- 不要在仓库新增按启动器划分的说明目录。支持范围写在**代码、测试和 `--inspect` 的实际输出**里。
- 不写“某版本一定支持”；只写“已在某哈希/某环境验证”，或明确“未验证”。
- 新启动器/新格式的需求走 **启动器支持请求** Issue 表单；附版本、文件类型、哈希、`--inspect` 输出和失败日志。
- README 只保留通用能力，不堆叠逐启动器细节。

## 4. 完成标准

- [ ] `cargo fmt --check` 通过
- [ ] `cargo test` 通过，含新增/修改的匹配测试
- [ ] `--inspect` 对目标类型和版本给出可解释结果
- [ ] `--dry-run` 不产生写入
- [ ] 至少一个“命中”和一个“跳过/失败”路径有测试
- [ ] 备份/恢复行为未被破坏
- [ ] 没有把启动器本体、账号数据、个人路径加入仓库
- [ ] 关键 UI 入口仍存在，且已做入口级检查
- [ ] 没有新增 `launchers/` 目录或未经验证的支持声明

## 5. 明确禁止

- DLL 注入、远程线程、运行时 hook、反调试/反注入绕过；
- 自动下载、持久化、隐藏进程或规避安全软件；
- 对未验证版本做“通用适配”承诺；
- 把未经证实的背景、动机或对抗性叙事写进代码注释、README 或 Issue 模板。

如果目标版本被混淆、加固或改成服务端限制，正确做法是**报告不支持**，而不是猜测或注入。

## 6. 发布

- Release 只上传压缩包（Windows `.zip`、Linux `.tar.gz`）。
- 不上传裸二进制、单独 README/SHA 或测试启动器。
- 详细步骤见 [`RELEASE.md`](RELEASE.md)。
