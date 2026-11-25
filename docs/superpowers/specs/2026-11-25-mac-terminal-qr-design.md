# Mac 终端字符二维码优化（0.2.3）

## 问题与参考

用户实际 macOS Terminal 截图中，黑色定位方框存在明显横缝。当前实现把黑色块字形画在浅色背景上，字形未覆盖字符格的部分会露出浅色背景。此前的字符还原解码只能验证矩阵映射，不能证明实际 Terminal 字体效果。

对照飞书官方安装工具 `@larksuite/openclaw-lark-tools@1.0.47`：`dist/utils/feishu-auth.js` 的 `printQRCode` 使用 `qrcode-terminal.generate(url, { small: true })`；`qrcode-terminal@0.12.0/lib/main.js` 紧凑模式使用深色背景和浅色半块字形，并采用紧凑边框。与 OpenClaw 内置飞书实现应区分。

官方说明：https://www.feishu.cn/content/article/7613711414611463386
官方包：https://registry.npmjs.org/@larksuite/openclaw-lark-tools/-/openclaw-lark-tools-1.0.47.tgz
参考依赖：https://registry.npmjs.org/qrcode-terminal/-/qrcode-terminal-0.12.0.tgz

## 实现

终端渲染采用背景填充优先：上下均黑或均白时用对应背景色和空格，整格不依赖字体；混合格用黑底、亮白前景的半块表示白色部分。连续相同背景合并 ANSI 切换。使用标准 16 色终端转义，避免引入字体、图片、依赖或假定终端支持真彩。

终端默认边距改为一模块，参考官方紧凑模式，使典型 37 模块二维码从 45×23 降为 39×20 字符。图形扫码页仍保留四模块静区，不改二维码编码与认证流程。紧凑字符版不是四模块静区的图形模式，两者应明确区分，不能宣称严格遵循完整静区规范。

配置项不增加，认证链接不默认打印。版本统一为 0.2.3（Debian 0.2.3-m5stack1）。

## 验证和限制

测试 ANSI 状态和字符组合恢复出的黑白模块；独立解码器验证短、典型、长载荷和明确的一/四模块边距输出。确认任意二维码行结束都会重置颜色，不污染后续 CLI。

当前 Computer Use 明确拒绝控制 com.apple.Terminal，不能绕过此限制，因此无法自动取得实际 Mac Terminal 截图。不能把合成图/矩阵还原说成真实终端截图，实际字体显示由用户复测。部署后清楚告知这一验证边界。掌机扫码页不改，运行时版本与包标识仍同步。

## 用户指定最终方案（覆盖上文候选）

用户要求直接采用飞书官方连接器使用的 qrcode-terminal small 模式。最终实现采用其相同的空格/浅色半块/全块映射、半格上边框、左右一格边框以及奇数行补白，不再加入自定义 ANSI 背景切换或强制配色，不增加图片备用入口或配置项。图形扫码页继续使用原来的标准像素绘制。删除未使用的可变 quiet_zone 渲染 API，不保留候选实现。

认证和 QR 矩阵编码仍为 Rust；只移植渲染布局，不引入 Node.js 运行时。保留 Apache-2.0 qrcode-terminal 的来源和许可证。用同一矩阵对照实际 npm qrcode-terminal@0.12.0 输出，验证字符与边框完全一致（忽略由 CLI 统一输出的末尾换行）；独立解码器再验证。Mac Terminal 原生 UI 仍无法由当前工具控制，不宣称已验证真实字体截图。

最终核验：12/92/244 字节样本的渲染逐字符匹配 npm 参考实现，并由独立解码器成功解码；对应尺寸 23×12、39×20、59×30。Mac 部署版本 0.2.3，个人配置 SHA-256 未变。原生 Terminal 的视觉效果尚未由工具验证。
