# 命令行参考

全局配置默认使用当前目录中的 `library.toml`：

```console
litman --config 路径 [--language system|en|zh-CN] 命令
```

普通表格、帮助、提示和错误可以本地化。供自动化使用的 JSON 字段名和枚举值始终保持英文和稳定。论文可以使用完整 UUID 或无歧义的 UUID 前缀指定。

`list` 和 `search` 的普通表格有意保持简洁，只包含短 ID、“标题”“第一作者”和“年份”。存在多位作者时，作者列显示第一作者并加“等”。表格不显示重要程度或文件可用性；如需查看单篇文献的完整详情，请使用 `show`，如需全部稳定字段，请使用 `--format json`。

```text
litman init --config FILE --root DIR [--language system|en|zh-CN]
litman scan [--refresh-metadata]
litman list [--group PATH] [--importance N] [--status present|missing|error] [--format table|json]
litman search QUERY [--group PATH] [--min-importance N] [--format table|json]
litman show PAPER_ID [--format table|json]
litman edit PAPER_ID [元数据选项] [--author VALUE ...] [--keyword VALUE ...] [--interactive] [--clear FIELD ...]
litman rate PAPER_ID 1|2|3|4|5|clear
litman group list
litman group create PATH
litman group rename PATH NAME
litman group move PATH [--parent NEW_PARENT|--root]
litman group delete PATH
litman group add PATH PAPER_ID ...
litman group remove PATH PAPER_ID ...
litman root set DIR
litman open PAPER_ID
litman backup DESTINATION
litman manual
```

重复的 `--author` 和 `--keyword` 会保留输入顺序。例如 `--clear title` 会设置一个明确的手工空值。GUI 中的“恢复 PDF 元数据”可删除覆盖并恢复内嵌元数据。`--interactive` 可在终端中依次填写字段。

示例：

```console
litman --config D:/library/library.toml scan
litman --config D:/library/library.toml search "机器学习" --min-importance 3 --format json
litman --config D:/library/library.toml edit 1b3a --title "修正后的标题" --author "李伟" --author "Ada Smith"
litman --config D:/library/library.toml group create "论文/方法"
litman --config D:/library/library.toml group add "论文/方法" 1b3a 57c2
litman --config D:/library/library.toml rate 1b3a 5
```

成功退出码为 0，命令语法错误为 2，找不到 ID 或 ID 有歧义为 3，其他文献库或 I/O 错误为 4。
