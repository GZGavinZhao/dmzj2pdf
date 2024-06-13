将动漫之家的漫画下载为PDF，有目录信息。

## 使用指南

```bash
cargo run -- <漫画id>
```
会将该漫画下载到`<漫画名>.pdf`文件中。

例如，《失色世界》的漫画ID为50749，那么执行`cargo run -- 50749`
则会将该漫画下载到`$PWD/失色世界.pdf`文件中。若文件已经存在，会进行覆写。

支持对下载文件重命名、并行下载等功能。具体用法请执行`cargo run -- help`
查看。请特别注意，为了减轻动漫之家服务器负担和防止被暂时封IP，并行下载不建议超过6。

具体下载某一话或几话理论可以实现，但没有精力去写这方面代码。请在知晓漫画目录后自行修改代码hardcode下载范围。

欢迎提交PR。

## 鸣谢

- [`flutter_dmzj`](https://github.com/xiaoyaocz/flutter_dmzj)
- [`dmzj`](https://github.com/cijiugechu/dmzj) Rust API
