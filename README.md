# Kawaks Rust 核心

跨平台 Rust 街机模拟核心，支持 CPS1 QSound 的 `dino`、`wof`、`wofa`、`punisher`，以及 CPS2 的 `ssf2` 和 Neo Geo 的 `kof97`。核心可无窗口运行，SDL2 窗口作为可选前端。从本地 ZIP 校验和装配 ROM，运行 68000 和 Kabuki Z80，并从 CPS1 图层、精灵和调色板生成 384×224 RGB 画面。QSound 合成器根据 Z80 写入的命令混合 16 个 PCM 声道，按 WinKawaks 1.65 的 22,050 Hz 设置，每帧输出 367 个 16 位立体声采样帧。需要当前稳定版 Rust。

## 运行

从本目录运行。以下路径对应项目位于 WinKawaks 目录中的布局，Windows、Linux 和 macOS 使用相同的命令。

```sh
cargo run --locked --release -- --rom ../roms/cps1/dino.zip --frames 120
cargo run --locked --release -- --game ssf2 --rom ../roms/cps2/ssf2.zip --frames 2600 --png ssf2.png --wav ssf2.wav
cargo run --locked --release -- --game kof97 --rom ../roms/neogeo/kof97.zip --load-state ../saves/kof97.ks0 --frames 24 --png kof97.png --wav kof97.wav
cargo run --locked --release -- --rom ../roms/cps1/dino.zip --load-state ../saves/dino.ks0 --frames 0 --png restored.png
cargo run --locked --release -- --rom ../roms/cps1/dino.zip --load-state ../saves/dino.ks0 --frames 120 --wav resumed.wav --png resumed.png
```

`--png` 导出最后一帧。`--frames 0` 导出刚恢复的画面，此时 WAV 有有效文件头和空音轨。`--wav` 从恢复点开始写入续跑期间的采样。输出路径应当尚未存在，其父目录应已创建。

KOF97 默认从游戏 ZIP 同目录读取 `neogeo.zip`，也可通过 `--bios` 指定路径。SSF2 使用含 `ssfe-03b` 的欧洲版。无窗口 CLI 使用空闲输入，嵌入程序可通过 `ArcadeMachine` 传入逻辑按键并读取画面和音频。

## 图形启动器、键盘和摇杆

点右上角的「中文」或 `English` 切换界面语言，下次启动会记住选择。游戏库显示中英文名称，可使用任一种名称或 ROM 短名搜索。选中游戏后，顶部会显示更完整的名称。

用稳定版 Rust、CMake 和平台的 C/C++ 工具链构建 SDL2 前端。Windows 如果使用 Visual Studio 2022 自带的 CMake，可在 PowerShell 设定 `$env:CMAKE_GENERATOR = 'Visual Studio 17 2022'`。

```sh
cargo run --locked --release --features sdl2 -- --gui
cargo run --locked --release --features sdl2 -- --window --game dino --rom ../roms/cps1/dino.zip
cargo run --locked --release --features sdl2 -- --window --game wofa --rom ../roms/cps1/wof.zip --load-state ../saves/wofa.ks0 --input-config input.cfg
```

启动器扫描本地的 CPS1、CPS2 和 Neo Geo ROM ZIP，可搜索名称或平台，并用箭头或鼠标滚轮翻页。带 N/A 的游戏仍缺兼容驱动。可用 `KAWAKS_ROM_DIR` 指定 ROM 根目录。启动器可以选择 ROM、Neo Geo BIOS、KWK4 存档和输入配置。文件对话框会从相关目录打开，也能从可执行文件附近查找默认 ROM。支持拖入 ROM 或存档，并可从窗口选择本地、主机或加入局域网。退出游戏窗口会返回启动器。启用 `sdl2` 构建后，无参数启动也会打开启动器。

- P1 键盘方向键、Z/X/C/V/B/N、1 开始、5 投币。P2 用 W/S/A/D、F/G/H/R/T/Y、2 开始、6 投币。本地游戏按 Enter 或小键盘 Enter 暂停和恢复，Esc 关闭窗口。局域网对战持续同步帧。
- 摇杆按连接顺序分配 P1–P3，支持热插拔。默认方向为 X/Y 轴 0/1 和帽开关 0，动作键 0–5，开始 7，投币 6。设备的实际编号可用配置覆盖。`--mute` 关闭窗口音频。
- 《三国志 II》的两款入口为 WOF WORLD（`wof`）和 WOF ASIA（`wofa`）。`--game wofa` 可读取合并包 `wof.zip` 中的亚洲版 ROM。该版本对应现有 `wofa.ks0` 与 `wofa.ks1`。


点击 `STARTING STAGE` 下的 `CHOOSE` 可选择起点。《三国志 II》的 WOF WORLD 和 WOF ASIA 提供开局原生选关，包括奖励关、分段和最终关。选择后按平时的方式投币、开始并选人，游戏会在确认角色后加载所选场景。也可以选择列出的本地 KWK4 槽位，或通过 `OPEN` 选文件。把 `<游戏ID>-stage-<名称>.kwk4` 放在本地 `saves` 目录即可添加自定义关卡检查点，内容应来自对应游戏和 ROM 版本。

点 `INPUT CONFIG` 下的 `CONFIGURE` 打开摇杆配置。选择 P1、P2 或 P3，再点击要修改的动作并按实体按钮；两个轴和方向帽可分别绑定。高亮显示当前映射收到的输入。重复绑定实体按钮时，配置窗口会交换原有绑定或解除冲突。还可清除绑定、调死区或恢复当前玩家默认值。`SAVE & APPLY` 保存到用户配置目录，下次从启动器打开会自动加载。原有输入文件可通过 `OPEN` 导入。

输入配置可以只覆盖需要调整的条目。用 `none` 禁用按键、轴、帽开关；键盘名称使用 SDL 扫描码名称。

```ini
p1.key.button1=J
p1.pad.button1=1
p1.pad.button2=0
p1.pad.axis_x=0
p1.pad.axis_y=1
p1.pad.hat=0
p1.pad.deadzone=10000
```

## 双人局域网

两边使用同一版本程序、同一个游戏 ROM 集，以及相同起始状态。图形启动器默认端口为 `22651`，可在地址栏粘贴 IP 和端口。主机控制 P1，客机的第一套键盘或摇杆控制 P2。以下示例用本机回环地址。跨设备时，主机填写自己的局域网监听地址，客机连接该地址。

```sh
cargo run --locked --release --features sdl2 -- --window --game dino --rom ../roms/cps1/dino.zip --net-listen 127.0.0.1:22651
cargo run --locked --release --features sdl2 -- --window --game dino --rom ../roms/cps1/dino.zip --net-connect 127.0.0.1:22651
```

联机使用双方每帧同步输入和状态 CRC，支持直接连接。Kaillera 协议、房间列表和互联网匹配尚待实现。

## 测试

```sh
cargo test --locked
cargo test --locked --test boot -- --ignored
```

第二条含本地 WOF/Punisher 启动、WOFA 两份存档，以及 SSF2 和 KOF97 从冷启动投币、选人到对战画面的回归。建议使用 `--release` 运行本地 ROM 测试。第一条验证 CPU、总线、Kabuki、存档解析、像素解码、PCM 混音及 PNG/WAV 编码。第二条需要本地 `roms/cps1/dino.zip` 与 `saves/dino.ks0` 到 `dino.ks3`，验证 Dino 冷启动、四份存档各自的恢复画面、首帧非静音双声道采样及续跑 60 帧。可用 `KAWAKS_FIXTURE_DIR` 指定含 `roms` 和 `saves` 的目录。CI 矩阵在 Windows、Linux 和 macOS 上运行第一组测试。macOS GUI 另在 Apple Silicon 和 Intel runner 上原生构建，启动 `--gui` 并检查 WindowServer 是否报告可见的 Cocoa 窗口。

在有图形会话的 Mac 上可以运行同一项检查。

```sh
cargo build --locked --release --features sdl2 --bin kawaks-rs
swift tools/check-macos-window.swift target/release/kawaks-rs
```

KOF97 的三份 KWK4 音频对照仍使用下文的本地 ROM、存档和 WinKawaks 采集文件。

## 对照 WinKawaks 1.65

在 Windows 上，从本目录运行以下命令。采集器使用 Windows SDK 的 x86 `cdb.exe`，在一个新目录里运行隔离的 WinKawaks 副本。每份存档从首次完整的模拟帧开始记录。采集文件可复制到 Linux 或 macOS 用同一个 Rust 测试比较。

```powershell
powershell -NoProfile -File .\tools\capture-win165.ps1 -Output .\outputs\win165-reference -Frames 24 -TraceSoundSlices
$env:KAWAKS_REFERENCE_DIR = (Resolve-Path .\outputs\win165-reference).Path
cargo test --locked --lib compare_win165_ -- --ignored --nocapture
```

测试比较每帧的 384×224 内部画面，将 Rust 的 RGB 像素量化为原版内部缓冲使用的分量位宽。PCM 比较使用 DirectSound 输出之前产生的双声道混音缓冲，并额外验证从 KWK4 重放原版 Z80 命令的采样。逐切片测试比较 Z80 在定时 IRQ 判定前后的 PC、实际周期、IRQ 相位和 IRQ 接收点。PCM 测试对齐本次采集的初始会话后，要求每帧 QSound 命令和采样完全一致。

IRQ 相位、上一切片的 IRQ 周期余量和 QSound 端口锁存没有写入 KWK4。WinKawaks 读档时沿用会话中的值，Rust 从 KWK4 创建新机器时将它们初始化为零。采集器保存这些值，可传入 `-SessionFrom <旧采集目录>` 重现已有采集的会话。原版的 Z80 定时包含两项已验证的兼容行为：DD/FD 索引指令不能在前缀处结束切片，LDIR/LDDR 按每字节 16 周期计算。

## 兼容范围

以下对照命令使用 WinKawaks 1.65 的本地副本。SSF2 先在原版投币并于有声音的标题画面保存到第 3 槽。KOF97 使用项目原有的第 0 槽比赛存档。

```powershell
powershell -NoProfile -File .\tools\capture-win165.ps1 -Game ssf2 -SaveRoot .\outputs\platform-baseline\saves -Slots 3 -Frames 24 -Output .\outputs\ssf2-coin-reference
powershell -NoProfile -File .\tools\capture-win165.ps1 -Game kof97 -Slots 0 -Frames 24 -Output .\outputs\kof97-animation-reference
cargo test --locked --release --lib compare_platform_win165_frames -- --ignored --nocapture
```

已采集的 SSF2 片段 24 帧画面完全一致。应用采集时的 QSound 会话相位和端口锁存后，17,616 个声道采样值也完全一致。该片段单独从 KWK4 启动时音频仍有差异。KOF97 第 0 槽比赛片段的 24 帧 304×224 可见区域和 17,616 个声道采样值均完全一致。旧档兼容混音恢复 ADPCM-A 和 Delta-T 的分数位置、预测器、步长及前一采样，按原版的 100 样本块处理 YM 写入和 Timer A 溢出。此片段用于验证七个 ADPCM 声道及混音。

```powershell
powershell -NoProfile -File .\tools\capture-win165.ps1 -Game kof97 -Slots 0 -Frames 24 -TraceNeoMix -Output .\outputs\kof97-ym2610-reference
cargo test --locked --release --lib adpcm_voices_match_win165 -- --ignored
```

逐声道测试分别比较 Delta-T 和六个 ADPCM-A 声道在最终量化前的混音贡献。采集输出目录可用 `KAWAKS_NEO_COMPONENT_DIR` 覆盖。

原版启动片段的 `kof97.ks1` 有游戏自然触发的 FM 输出。`kof97.ks2` 保留同一 FM 场景并用原版 PSG 产生诊断方波。两份档各核对 24 帧的 FM 相位、步长、包络阶段和位置，PSG 计数器及输出相位，以及 17,616 个立体声混音值，均与 WinKawaks 1.65 一致。诊断档还验证了 4,795 个有声 PSG 采样帧。原版冷启动和演示扫描记录了 441 次 FM 键控，额外检查了第 0 槽的比赛场景。这些扫描未捕获游戏自然触发的 PSG 非零音量写入。

```powershell
& .\tools\capture-win165.ps1 -Game kof97 -SaveRoot .\outputs\kof97-active-probe\saves -Slots 1,2 -Frames 24 -TraceNeoSources -Output .\outputs\kof97-active-audio
cargo test --locked --release --lib compare_active_kof97_audio -- --ignored --nocapture
```

用 `KAWAKS_NEO_ACTIVE_DIR` 指定含存档和原版采样快照的目录。

- KWK4 导入识别 WinKawaks 1.65 的 CPS1 QSound、CPS2 和 Neo Geo 布局。KWK4 缺少游戏标识，应使用由所选游戏及区域 ROM 版本生成的存档。
- PCM 恢复按本机存档使用的 22,050 Hz 解释旧混音器的采样步长。CPS QSound 以 WinKawaks 的 PCM 混音路径为基准，DSP 回声、滤波和 QSound ADPCM 尚未实现。CPS2 的光栅中断和 Neo Geo 的其他卡带与显示模式仍缺少验证。
- 精灵表在 CPU 执行前锁存，调色板基址在帧尾锁存，绘制时从当前图形 RAM 读调色板。PNG 保留 RGB888，差分测试使用原版缓冲的 5 位分量。
- ROM 和 KWK4 从本地读取。示例输出由本地夹具生成，版本控制忽略 `outputs/`。

参见 [第三方来源与许可证](THIRD_PARTY.md)。
