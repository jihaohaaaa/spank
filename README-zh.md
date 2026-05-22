<p align="center">
  <img src="doc/logo.png" alt="spank logo" width="200">
</p>

# spank

[English][readme-en-link] | **简体中文**

拍打你的 MacBook，它会回应你。

> "这是我见过的最神奇的东西" — [@kenwheeler](https://x.com/kenwheeler)

> "我刚才在妻子旁边运行了性感模式...笑死我们了" — [@duncanthedev](https://x.com/duncanthedev)

> "巅峰工程" — [@tylertaewook](https://x.com/tylertaewook)

使用 Apple Silicon 芯片的加速度传感器，检测笔记本电脑的物理撞击并播放音频回应。单个 Rust 二进制文件，无独立守护进程。

## 系统要求

- 基于 Apple Silicon 的 macOS（任何 M2 及以上型号的 M 系列芯片，或特定的 M1 Pro 型号，不包括其他 M1/A 系列芯片！）
- `sudo`（用于访问加速度传感器）
- 带 Cargo 的 Rust 工具链（用于安装）

## 安装

使用 Cargo 安装：

```bash
cargo install --git https://github.com/taigrr/spank spank
```

在本地检出目录中安装：

```bash
cargo install --path .
```

> **注意：** `cargo install` 默认会将二进制文件放在 `~/.cargo/bin`。将其复制到系统路径以便 `sudo spank` 能够工作：
>
> ```bash
> sudo cp "$HOME/.cargo/bin/spank" /usr/local/bin/spank
> ```

## 使用方法

默认运行会打开交互式终端 UI，显示当前模式、设置、最近一次拍打和事件日志。音效包和运行时设置都可以在 TUI 内修改；CLI 参数只用于设置初始状态或选择 `--stdio` 集成模式。

TUI 控制：

- `q`、`Esc` 或 `Ctrl-C`：退出
- `Space` 或 `p`：暂停/继续检测
- `v`：切换音量缩放
- `Up`/`Down`：选择控制项
- `Left`/`Right`：调整选中的控制项
- `Enter`：应用/切换/编辑选中的控制项
- `m`：切换音效包
- `f`：切换默认/快速调优
- `e`：编辑自定义音频来源

自定义来源可以是 MP3 目录，也可以是用逗号分隔的 MP3 文件列表。

```bash
# 打开 TUI；在界面里切换模式和设置
sudo spank

# 可选初始状态
sudo spank --sexy
sudo spank --halo
sudo spank --fast
sudo spank --sexy --fast
sudo spank --custom /path/to/mp3s
sudo spank --custom-files /path/a.mp3,/path/b.mp3
sudo spank --min-amplitude 0.1
sudo spank --cooldown 600
sudo spank --speed 0.7

# JSON stdin/stdout 模式，用于脚本、launchd 和其他集成
sudo spank --stdio
```

### 模式

**疼痛模式**（默认）：检测到拍打时随机播放 10 个疼痛的音频片段之一。

**性感模式**（`--sexy`）：监听在 5 分钟滚动窗口内的拍打次数。拍打越多，音频回应越强烈。60 个升级级别。

**光环模式**（`--halo`）：检测到拍打时随机播放光环视频游戏系列的死亡音效。

**自定义模式**（`--custom`）：从你指定的自定义目录中随机播放 MP3 文件。

### 检测调优

在 TUI 中切换快速配置可获得更快的轮询（4ms vs 10ms）、更短的冷却时间（350ms vs 750ms）和更大的样本批次（320 vs 200）。

运行时可以直接调整 `min-amplitude`、`cooldown`、`speed`、`volume-scaling`、音效包和自定义音频来源。

### 灵敏度

使用 `--min-amplitude` 控制检测灵敏度（默认：`0.05`）：

- 较低值（例如 0.05-0.10）：非常敏感，检测轻拍
- 中等值（例如 0.15-0.30）：平衡的灵敏度
- 较高值（例如 0.30-0.50）：只有强烈的撞击才会触发声音

该值表示触发声音所需的最小加速度幅度（以 g 为单位）。

## 作为服务运行

要让 spank 在启动时自动运行，请创建一个`系统守护进程`配置文件。服务应使用 `--stdio`，因为默认模式是交互式 TUI。选择运行模式，如下：

<details>
<summary>疼痛模式（默认）</summary>

```bash
sudo tee /Library/LaunchDaemons/com.taigrr.spank.plist > /dev/null << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.taigrr.spank</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/spank</string>
        <string>--stdio</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/spank.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/spank.err</string>
</dict>
</plist>
EOF
```

</details>

<details>
<summary>性感模式</summary>

```bash
sudo tee /Library/LaunchDaemons/com.taigrr.spank.plist > /dev/null << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.taigrr.spank</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/spank</string>
        <string>--stdio</string>
        <string>--sexy</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/spank.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/spank.err</string>
</dict>
</plist>
EOF
```

</details>

<details>
<summary>光环模式</summary>

```bash
sudo tee /Library/LaunchDaemons/com.taigrr.spank.plist > /dev/null << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.taigrr.spank</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/spank</string>
        <string>--stdio</string>
        <string>--halo</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/spank.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/spank.err</string>
</dict>
</plist>
EOF
```

</details>

> **注意：** 如果你将 spank 安装在其他位置（例如 `~/.cargo/bin/spank`），请更新 spank 的路径。

加载并启动服务：

```bash
sudo launchctl load /Library/LaunchDaemons/com.taigrr.spank.plist
```

由于 `plist` 文件位于 `/Library/LaunchDaemons` 且未设置 `UserName` ，`launchctl` 命令会以 root 身份运行它， 所以不需要加 `sudo`。

要停止或卸载：

```bash
sudo launchctl unload /Library/LaunchDaemons/com.taigrr.spank.plist
```

## 工作原理

1. 通过 Apple Silicon 芯片的加速度传感器直接读取原始加速度传感器数据
2. 运行振动检测（瞬态/长时瞬态、累积偏差、峰度、峰值/平均绝对偏差）
3. 在 TUI 中显示实时状态、设置、最近一次拍打详情和事件历史
4. 当检测到显著撞击时，播放嵌入的 MP3 回应
5. **可选 JSON 模式**（`--stdio`），用于脚本、launchd 和集成
6. **可选音量缩放**（`--volume-scaling`）— 轻拍时安静播放，重拍时以全音量播放
7. **可选速度控制**（`--speed`）— 调整播放速度和音调（0.5 = 半速，2.0 = 2倍速）
8. 有 750ms 响应冷却时间以防止快速连续播放，可通过 `--cooldown` 调整

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=taigrr/spank&type=date&legend=top-left)](https://www.star-history.com/#taigrr/spank&type=date&legend=top-left)

## 致谢

传感器读取和振动检测来源于 [olvvier/apple-silicon-accelerometer](https://github.com/olvvier/apple-silicon-accelerometer)。

## 许可证

MIT

<!-- Links -->
[readme-en-link]: ./README.md
