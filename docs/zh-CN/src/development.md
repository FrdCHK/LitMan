# 开发手册

## 架构

LitMan 是由三个 crate 组成的 Cargo workspace。`litman-core` 负责全部策略和数据访问；两个界面均不直接写 SQL 或解析 PDF。`litman-cli` 把稳定命令映射到核心服务。`litman-gui` 使用 eframe/egui，嵌入 Noto Sans CJK SC 字体，并在线程中扫描。

依赖策略优先选择纯 Rust 与内置组件。SQLite 通过 `rusqlite` 静态捆绑，`lopdf` 读取 PDF，`quick-xml` 解析 XMP；可选的 ADS/SciXplorer HTTPS 客户端使用 `ureq` 与 Rustls。不使用 Python、Node.js、JVM、数据库服务器或遥测。新增依赖前必须审查许可证、维护状况、最低平台、二进制大小和安全性。

## 配置与路径

`Config::load` 要求模式版本为 1，且数据库文件直接位于配置文件旁。相对 `library_root` 从配置目录解析。`relative_pdf_path` 会规范化文件与根目录、确认文件仍在根目录中，并使用 `/` 序列化路径组件。扫描器不跟随符号链接。

不得直接把数据库中的 PDF 路径用于写操作。所有文件解析必须经过配置根目录；PDF 是不可变输入边界。

## 数据模型与迁移

SQLite 连接启用外键、写入繁忙超时和 `journal_mode=DELETE`，因此关闭后数据库是单个可移植文件。`PRAGMA user_version` 标识迁移。每个迁移在事务中执行；必须新增向前迁移，不能改写版本 1。

`papers` 保存 UUID、规范化相对路径、大小、纳秒修改时间、BLAKE3 哈希、存在/缺失/错误状态、诊断、时间戳、最终字段、原始内嵌元数据 JSON、手工字段集合、可选的原始 BibTeX/bibcode 与 BibTeX 字段来源集合，以及受 1–5 约束的可选重要程度。作者和关键词是有序 JSON 数组。`groups` 使用邻接表并保证同级名称唯一；`paper_groups` 实现多对多关系，记录或分组删除时级联。

模式变化必须保持从所有受支持系统复制来的数据库可用。新增迁移测试应从前一 `user_version` 开始，执行升级并验证外键和约束。

## 元数据与扫描协调

提取器读取 PDF 尾部 Info 字典和 XMP 元数据流。识别 XMP、Dublin Core 与 PRISM 等命名空间；未知原始值保留用于诊断。BibTeX 导入会解析 ADS 引用键和常见书目信息，同时原样保留完整导出。每个字段采用最近一次手工或 BibTeX 覆盖，其后依次为 XMP、PDF Info 和空值。手工字段集合会保护用户值和 BibTeX 值不被扫描覆盖；独立的 BibTeX 字段集合用于准确显示来源，之后手工修改某字段时会清除其 BibTeX 来源。

增量扫描比较大小与修改时间，对变化文件计算哈希，并按需提取元数据。具有相同哈希且唯一的缺失记录会被视为移动；多条同哈希记录只是标记为副本，绝不合并。未观察到的文件改为缺失但保留元数据。解析失败仅影响相应记录。

## 搜索与本地化

搜索从最终字段、数组、备注、路径、分组与重要程度构建文本，查询和文本都进行 Unicode NFKC 及小写转换，然后做字面子串匹配。v1 不应加入依赖语言的分词器。

共享 `Locale` 将用户文本键映射到英文和简体中文。JSON 模型、数据库枚举、字段名和命令名保持英文。每次新增文本必须同时添加两种翻译。测试中文输入和字形；GUI 应在首帧前加载字体。

## GUI 线程与无障碍

GUI 在界面线程中持有一个 `Library`。扫描工作线程根据配置打开独立连接，通过通道发送有界进度消息；SciXplorer 工作线程只接收克隆的 API 客户端及查询/bibcode，再通过通道返回数据，SQLite 写入仍留在界面线程。HTTPS 调用具有 30 秒全局超时。每个文件之间检查原子取消标记。输入框应有文字标签，保留键盘导航与焦点指示，不能只靠颜色表达操作。

Windows 构建同时包含 WGPU 和 Glow，启动时优先 WGPU，失败后用 Glow 重试。Linux 发布构建仅用 X11/Glow，以保持 CentOS 基线。

## 测试与样例

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

单元测试覆盖配置/路径、迁移、元数据优先级、手工覆盖、重要程度、嵌套分组、Unicode/中文搜索和扫描协调。PDF 样例必须覆盖 Info、XMP/PRISM、中文字段与文件名、无元数据、冲突、损坏/加密、副本、移动和缺失。CLI 集成测试断言 JSON 键与本地化表格。GUI 测试应先隔离视图模型逻辑，再进行平台启动、键盘访问、取消和系统打开 PDF 的冒烟测试。

运行 `cargo run -p litman-core --example generate_fixtures -- target/litman-fixtures` 可生成确定性的完整样例集。`SCENARIOS.txt` 说明移动与缺失测试所需的重命名/移除步骤，并记录加密样例密码。

每次验收都必须在操作前后对所有样例 PDF 计算哈希；任何变化都会阻止发布。

## 安全与贡献流程

PDF 内容、元数据、BibTeX 和网络响应都应视为不可信输入。保留解析限制，避免 shell 插值，验证路径及 bibcode，使用参数化 SQL，限制网络调用，并把错误作为普通文本显示。不得记录或显示已配置的 bearer 令牌。令牌按设计仅以明文保存在可移植 TOML 中，因此文档必须提醒用户注意配置副本、备份、shell 历史和源码管理。安全问题应私下报告给发行渠道列出的维护者。

贡献应使用聚焦分支，完成格式化、测试、两种翻译和行为相关文档。合并前重点审查迁移、文件系统边界、JSON 兼容性、平台依赖以及 PDF 不可变性。
