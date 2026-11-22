# Colibri 拼音词库

`words.tsv` 是独立的词库数据，按 GPL-3.0 分发。原始数据是 [雾凇拼音](https://github.com/iDvel/rime-ice/tree/927e1cf1d3eeb9bf4be85e3f684dba4dd485b6f6/cn_dicts) 在提交 `927e1cf1d3eeb9bf4be85e3f684dba4dd485b6f6` 的 `8105.dict.yaml` 和 `base.dict.yaml`。原 YAML 头保留上游数据来源与致谢。

下载上述原始文件后运行 `python3 generate.py 8105.dict.yaml base.dict.yaml > words.tsv`。转换筛选低频词并按全拼与词频排序，保留音节边界。
