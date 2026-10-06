# kson-rs iPadOS 移植与更改记录

- 仓库：`123xzxc/kson-rs`（`Drewol/kson-rs` 的 fork，PUBLIC）
- 移植分支：`feat/ipados-port`（本地分支 `codex/ipados-port`）
- 路线：**路线 A** —— 用 GitHub Actions 编译，产出无签名 IPA，LiveContainer 侧载
- 目标系统：iPadOS / iOS 15.0+（`TARGETED_DEVICE_FAMILY = 2`）
- 本文档对应当前分支状态（`25098aa` 之后加入了歌曲来源切换）

---

## 1. 背景与目标

上游 `kson-rs` 是一个用 Rust 写的 SDVX（Sound Voltex）类音游，原本只有桌面端
（winit + femtovg/OpenGL + gilrs）。目标是把它移植到 iPadOS：

1. 不改动游戏逻辑，尽量把平台相关代码收敛到新的 `game/src/platform/` 下。
2. 复用已有的 Lua 皮肤系统（`game/skins/Default`），让菜单、选曲、设置全部走原皮肤。
3. 在没有 macOS 的环境下开发：Rust 侧可本地交叉 `cargo check`，ObjC/Xcode 侧只能靠 CI。
4. 用 GitHub Actions 产出无签名 IPA，不依赖签名证书。

---

## 2. 总体架构

### 2.1 组成

| 层 | 位置 | 说明 |
| --- | --- | --- |
| ObjC 应用壳 | `ios/KsonGame/Classes/` | `UIApplication` + `UIView`，驱动 Rust 的 `init/frame/resize/touch` |
| Rust 静态库 | `game/src/`（`crate-type = staticlib`） | 游戏本体，通过 `extern "C"` 入口暴露给 ObjC |
| iOS 平台层 | `game/src/platform/` | 渲染、触摸、手柄、路径、时间等平台实现 |
| 手柄桥 | `ios/KsonGame/Classes/KsonGamepad.m` | `GameController.framework` → Rust 手柄事件 |
| gilrs 替身 | `ios/gilrs-stub/` | iOS 上没有 gilrs，提供一个同接口的 stub crate |
| 工程生成 | `ios/project.yml` | XcodeGen 配置，CI 里生成 `KsonGame.xcodeproj` |
| 皮肤 | `game/skins/Default/` | 原 Lua 皮肤，运行时从容器目录读取 |

### 2.2 渲染

- `game/src/platform/render.rs`：把 femtovg 指向 EAGL 的 framebuffer，并补上
  depth/stencil attachment；`framebuffer 0` 被重定向到 drawable。
- `game/src/platform/app.rs`：主循环。每帧
  `game.update()` → `flush_pending_touches()` → `render_ios()`（皮肤 canvas + 虚拟控制器 overlay）
  → `egui_host` 画设置界面 → 呈现。
- `game/src/egui_host.rs`：iOS 上的 egui 用 `egui_glow` 画（桌面的 `egui-winit` 不可用）。
- 所有 GL 错误带阶段名上报（`render.rs` 的 `drain_error`），例如
  `frame N: gl error 0x500 at stage \`frame-start\``。

### 2.3 输入路由

```
UIKit 触摸 ──► kson_ios_touch ──► IosApp::route_touch
                                    ├─ egui 界面（设置）        → egui 事件
                                    ├─ 菜单场景（touch_as_mouse）→ 合成鼠标事件 + 拖动转旋钮
                                    └─ 游戏内                   → 原始触摸网格（激光手势）
物理手柄 ──► GameController ──► kson_ios_gamepad_* ──► GamepadEvent 队列 ──► drain() ──► Laser/Button 事件
虚拟面板 ──► IosTouchState::update ──► Laser/Button 事件（同上）
```

关键点：

- **皮肤菜单需要鼠标事件**（悬停、点击），所以 iOS 触摸会被合成成
  `CursorMoved` / `MouseInput` 再喂给皮肤。
- **触摸坐标要换算成渲染像素**（`game_main` 里的缩放），否则皮肤命中区全部错位。
- **游戏内**才用原始触摸网格，拖动即激光。

---

## 3. 构建与发布

### 3.1 GitHub Actions

工作流：`.github/workflows/ios.yml`（单 job `build`，只监听 `master` / `feat/**`）。

流程：`cargo rustc --release --target aarch64-apple-ios --crate-type staticlib`
→ XcodeGen 生成工程 → `xcodebuild` 出 `.app` → 打包成无签名 `.ipa` → 上传 artifact `USC-ios-unsigned`。

构建参数（见 `ios/project.yml` 的 preBuildScript）：

```
cargo rustc --release --target aarch64-apple-ios -p rusc --lib \
  --crate-type staticlib \
  --features embed-assets --no-default-features \
  -- -C strip=debuginfo -C link-dead-code
```

- `--no-default-features`：把桌面专用的 SoundTouch（C++）排除掉。
- `embed-assets`：把 `skins/`、`fonts/` 打进二进制，首次启动解包到容器目录。
- `-C strip=debuginfo` + `-C link-dead-code`：保留符号表，让强制加载符号生效。

### 3.2 链接注意事项（踩过的坑）

Rust 静态库里的入口函数不会被 ObjC 全部引用，链接器默认会丢弃。因此：

1. `game/src/lib.rs` 顶层有 `mod ios_exports`，**每个 FFI 入口都要在这里再导出一次**，
   否则静态库里没有该符号。
2. `ios/project.yml` 里 **两处**都要加 `-u _kson_ios_xxx`：
   `FORCE_LOAD_SYMBOLS` 和 `KsonGame` target 的 `OTHER_LDFLAGS`。
3. `librusc.a` 在 `OTHER_LDFLAGS` 里显式列出，只写 `-lrusc` 会在库搜索路径未就绪时被静默丢弃。

当前入口：

```
kson_ios_init / kson_ios_frame / kson_ios_resize / kson_ios_touch
kson_ios_gamepad_button / kson_ios_gamepad_axis / kson_ios_set_controllers
kson_ios_capture_gamepad_button / kson_ios_capture_gamepad_axis
kson_ios_axis_binding / kson_ios_knob_axis
```

### 3.3 本地可做的检查（Windows，无 macOS）

```powershell
# iOS 交叉 check（zig shim 提供 clang/ar）
$shim = "<repo>\target\zig-shim"
$env:Path = "$shim;$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CC_aarch64_apple_ios="$shim\clang.bat"; $env:CXX_aarch64_apple_ios="$shim\clang.bat"
$env:AR_aarch64_apple_ios="$shim\ar.bat"; $env:SDKROOT="$shim\sdkroot"
$env:IPHONEOS_DEPLOYMENT_TARGET="15.0"
cargo check -p rusc --lib --no-default-features --target aarch64-apple-ios `
  --features embed-assets --message-format short

# 桌面 check（保证没有破坏原有平台）
cargo check -p rusc --lib --no-default-features --message-format short
```

ObjC 侧无法本地编译，只能靠 CI 验证。

---

## 4. 更改记录

### 4.1 启动与崩溃

| 提交 | 问题 | 修复 |
| --- | --- | --- |
| `acae2e8` | 首次启动闪退 | 解包 `embed-assets`；缺字体不再 panic |
| `3c1ef7e` | 服务注册与沙盒路径错误 | 修正 iOS 的服务注册、`Documents/USC` 路径解析 |
| `18d8acd` | 呈现时 GL 报错 | 呈现前绑定 drawable renderbuffer |
| `be50050` | canvas 服务缺失 | 重新注册裸 canvas 服务 |
| `b74a8b9` | 启动进错场景 | 直接进标题界面，并打上歌曲启动日志 |
| `3818dbc` | 全新安装时配置存不下来 | 保存配置时先建目录 |
| `2e68ab7` | egui 输入无效 | iOS 的 egui 事件之前根本没进 context |
| `5875835` | Get Songs 闪退 | jacket 图改走 async runtime；Lua `print` 转发；`LuaHttp` 回调日志 |

### 4.2 渲染

| 提交 | 说明 |
| --- | --- |
| `be11d94` | 通过 `OpenGLES.framework` 解析 GLES 入口 |
| `2748d13` | 把 framebuffer 0 重定向到 EAGL drawable，修正 layer scale |
| `5aab07a` | femtovg 指向 EAGL framebuffer，并补 depth/stencil |
| `18c78d7` | 使用 core ES3 的 depth/stencil 枚举 |
| `0b05998` / `18d8acd` | 从呈现侧重新绑定 drawable framebuffer |
| `1387b11` | GL 错误带阶段名，便于定位 |
| `47b387b` | egui 界面改用 `egui_glow` 绘制 |

已知残留（无害，未处理）：皮肤背景 shader 的 GLSL ES 3.0 编译失败（只影响背景层）、
偶发 `gl error 0x500` / `egui_glow` `GL_INVALID_OPERATION 0x502`。

### 4.3 触摸

| 提交 | 问题 | 修复 |
| --- | --- | --- |
| `69f9d2d` | 菜单点不动 | iOS 菜单触摸合成为鼠标事件；拖动转成旋钮 |
| `21b9bfb` | 命中区错位 | 触摸坐标换算成渲染像素 |
| `04836d6` | 皮肤菜单看不到 hover | 延迟投递 iOS 触摸，先给皮肤 hover |
| `ab61bd0` | 面板位置/大小/旋转错误 | 修正布局与旋转重算 |
| `2e68ab7` | egui 输入丢失 | 事件送达 context |
| `29f15d4` | 菜单上按键可见但点不动 | 触摸先判定「按面板」还是「拖菜单」，整个手指生命周期内保持一致 |
| `29f15d4` | 标题界面被面板挡住 | 主菜单自动隐藏面板（含左上角切换钮） |

### 4.4 虚拟控制器（屏幕面板）

| 提交 | 说明 |
| --- | --- |
| `5416dd4` | 加入可隐藏的虚拟按键层 |
| `998d107` | 按 SDVX 控制器风格排版 |
| `d0dc84c` | 放大面板 |
| `ab61bd0` | 修正位置、尺寸、旋转、旋钮 |
| `29f15d4` | BT 键加大、FX 条下移放大、BT 加 A/B/C/D 字母、FX 加方向箭头 |
| `29f15d4` | 主菜单自动隐藏；设置里可手动隐藏 |

布局参数在 `game/src/touch.rs::TouchHelper::new`，绘制在 `game/src/platform/input.rs`
（`paint_overlay` / `paint_toggle` / `pentagon`）。

### 4.5 手柄 / 手台（PHAC）

用户使用的是 PHAC 自制手台：**旋钮模拟左摇杆 X/Y，腰杆不回中，无右摇杆**。

| 提交 | 问题 | 修复 |
| --- | --- | --- |
| `5416dd4` | 无手柄支持 | 接入 `GameController.framework` |
| `d3bf1fa` | 手柄按键无法配置 | 打通控制器绑定界面 |
| `b2ff0cd` / `fa07036` | 新入口没被链接 | 强制加载 + 从 crate root 再导出 |
| `45e4352` | 旋钮在游戏内无效 | 修绑定查找与输入路径 |
| `5f5f473` | 旋钮游戏内可用 | 绑定持久化到配置 |
| `0c1c7c3` | 轴与 PHAC 不一致 | 按固件（左摇杆 X=左旋钮、Y=右旋钮）修正 |
| `2699225` | 载入到旧的坏绑定 | 换持久化 key；恢复轴灵敏度 |
| `21ca37b` | 旋钮又不动了 | 每个事件重置 delta，避免累加把激光顶死 |
| `0e61ea9` | 方向反、太灵敏 | 翻转实体旋钮方向并降低灵敏度 |
| `6d5a07f` | 灵敏度不可调 | 设置里加 Knob sensitivity（0.05–3.0） |
| `29f15d4` | **一个旋钮同时带动左右两边激光** | 见下方「轴捕获」 |
| 本次 | **一个旋钮仍然会带动另一边激光** | 见下方「轴串扰」 |

#### 轴串扰

现象：`29f15d4` 之后绑定已经是 `axis 0` / `axis 1`，但打歌时转任意一个旋钮，
**两边激光仍然一起动**（另一边幅度约是被转那边的 5%–20%）。

日志证据（`ios(17).log`）：

```
knob Left  axis=0.4365 step=0.0326 other=-0.0993
knob Right axis=-0.0993 step=0.0000 other=0.4365
knob Left  axis=0.4524 step=0.0160 other=-0.3935
knob Right axis=-0.3935 step=-0.2942 other=0.4524
```

转右旋钮时右轴每包走 ~0.24（约 3 个 detent），而**没被转的左轴也在走 0.02–0.03**。
固件每个包会把摇杆的**两个轴一起采样上报**，没被转的那个轴读数会漂移一点点；
这个漂移刚好超过 Rust 侧 `AXIS_STEP_DEADZONE`（0.01），于是也生成了一次激光事件。

修复：`KsonGamepad.m::feedKnobsX` 里加**主轴判定**——
同一包里两个轴都动了时，位移小的那个丢掉，除非两者量级接近（`KsonKnobAxisDominance = 2.5`，
对应「两个旋钮一起转」）。被转的轴位移是闲置轴的数倍，所以能干净分开，
而慢速转动不会因为阈值被吞掉（判据是比值，不是绝对阈值）。

#### 轴捕获（`29f15d4`）

现象：打歌时转手台旋钮，**左右两条激光一起动**；设置里左右旋钮、甚至 Back 都显示 `axis 1`。

根因：`KsonGamepad.m` 的 `captureAxes` 在**每次轴事件**里按固定顺序把四个轴全部报给设置界面，
第一个 `LeftY` 永远先命中，于是所有绑定都写成 `axis 1` → 两个旋钮同轴。

修复：

- 只上报**真正位移的轴**（阈值 0.02），斜推时取位移更大的那个。
- 忽略主机把左摇杆值镜像上报成右摇杆的情况。
- 加载/写入绑定时，若两个旋钮指向同一轴，自动清掉其中一个（回落默认 X/Y）。
- 绑定 key 换新（`...6403`），旧的坏绑定直接作废。

### 4.6 Get Songs / 谱面

| 提交 | 问题 | 修复 |
| --- | --- | --- |
| `ba18f48` | 主菜单 Downloads 打不开 | 新增 `game/src/download_screen.rs` + 皮肤 `downloadscreen.lua` |
| `5e38479` | 皮肤用 `Http.GetAsync` 但全局名是 `http` | 注册 `Http` 别名 |
| `45e4352` | Get Songs 闪退 | 修 Lua 服务与 jacket 加载 |
| `6d5a07f` | 卡在 LOADING | 修 HTTP 轮询 |
| `5875835` | 进得去但下载闪退 | jacket 改 async runtime；Lua 错误上报 |
| `29f15d4` | 界面无法操作 | 下载界面把鼠标按压转发给脚本：点条目选中，再点一次下载 |
| `25098aa` | **点 Start / 双击谱面不下载** | 见下方「dlScreen 绑定」 |
| `25098aa` | **旋钮无极、定不到谱面上** | 见下方「旋钮步进」 |
| `25098aa` | 谱面目录找不到 | 启动日志打印目录与条目数，并写入 `PUT_CHARTS_HERE.txt` 标记 |
| `0d9eb10` | 切换歌曲来源要按两次 | 见下方「来源切换要按两次」 |
| 本次 | **每下载一次都会刷新下载界面** | 见下方「下载后不再重建 Lua」 |
| 本次 | 下载界面没有返回键、其他虚拟键点了没反应 | 见下方「下载界面的触摸」 |
| 本次 | 下载界面不能拖动翻页 | 见下方「下载界面的触摸」 |
| 本次 | **滑动是「跳格」不是跟手** | 见下方「跟手翻页」 |
| 本次 | **等级 / 排序没有触摸入口** | 见下方「等级 / 排序按钮」 |
| 本次 | **切到本地列表不刷新** | 见下方「本地列表刷新」 |

#### dlScreen 绑定（`25098aa`）

现象：报错 `bad argument #1 to DlScreenLua.DownloadArchive: error converting Lua string to
userdata`，Start 和双击都不下载。

根因：`dlScreen` 用 `UserData::add_function` 注册，函数签名第一个参数是 `this: AnyUserData`，
需要 `dlScreen:DownloadArchive(...)` 调用；但皮肤（和上游一致）写的是 `dlScreen.DownloadArchive(...)`，
于是 URL 被当成 `this`，参数整体错位一位。

修复：把 `dlScreen` 从「带方法的 userdata」改成**普通 table + 闭包**，
状态（channel、路径、mixer）在注册时被闭包捕获，调用风格与 `Http.GetAsync` 一致（`.`）。

#### 旋钮步进（`25098aa`）

现象：Get Songs 里旋钮是「无极」的，高亮一直在滑，**永远停不到某一张谱面上**。

根因：`tick` 把旋钮的**原始模拟增量**（一个 detent 的零头）直接传给 Lua 的
`advance_selection(steps)`，`cursorPos = (cursorPos + steps) % #songs` 因此停在分数索引上。

修复：在 Rust 侧累加，**每满 3 个 detent 才走一格**，余数留到下一帧；
阈值按 `knob_sensitivity` 归一化，所以灵敏度设置不会改变「几格一步」。

#### 谱面目录

游戏读取的目录是 `<NSHomeDirectory()>/Documents/USC/songs`，由
`GameConfig::songs_path`（默认 `./songs`）相对 `game_folder` 解析而来。

导入器（`game/src/song_provider/files.rs`）会**递归**扫描该目录，任何包含
`.ksh` / `.kson` 的文件夹都算一首歌；Get Songs 下载的谱面解包到
`songs/nautica/<id>/`，因此也会被扫到。

> LiveContainer 下这个路径是多层 UUID 嵌套，例如：
> `/var/mobile/Containers/Data/Application/<A>/Documents/Data/Application/<B>/Documents/USC/songs`。
> 启动日志会打印 `Charts folder: <路径> (N entries)`，N 就是该目录里的条目数。

#### 歌曲来源切换（本地 / 在线）

「开始游戏」一次只列一个来源，所以选曲界面的设置对话框里加了一个 **Song Provider** 标签页
（和偏移、判定设置放在一起，双 FX 键打开）：

| 选项 | 内容 |
| --- | --- |
| `Local Files` | `songs_path`（默认 `Documents/USC/songs`）里的谱面，含 Get Songs 下载的 |
| `Nautica (Online)` | ksm.dev 的在线目录 |

选择写进 `GameConfig::song_select.provider`，重启后仍然生效，主菜单按它决定初始列表。

实现要点：

- 设置对话框在场景拿到 `program_control` **之前**就建好了，所以标签页先通过自己的 channel
  上报选择，由 `SongSelectScene::tick` 转发成 `ControlMessage::SongSelect`。
- 该消息原来用 `Scenes::suspend_top()`，会把旧选曲界面留在场景栈里（每切一次多一层，
  Back 会一层层退回历史列表）。现在改成 `Scenes::pop_top()`，让界面**替换**自己。
- `SettingsDialog::push_tab` 用来在对话框建好之后再追加标签页。

##### 来源切换要按两次（`0d9eb10` 遗留）

现象：设置里 Song Provider 标签页显示 `Nautica (Online)`，点一次还是 Online，
点两次才变 `Local Files`。

根因：标签页的 `get` 闭包捕获的是**界面创建时的快照** `provider_index`，
而 `change_setting` 是 `set((get() + steps).rem_euclid(len))`。第一次按下时
`get()` 仍是旧值，算出来的目标值和当前值相同，于是显示不动；
但 `set` 已经写了配置、场景也重进了一次，第二次按下才从新的快照算出可见的变化。

修复：`get` 改成**实时读** `GameConfig::get().song_select.provider`，按一次即可切换，
显示也立刻跟上（`on_button_press` 结束时会重新把 `SettingsDiag` 塞回 Lua）。

##### 下载后不再重建 Lua

现象：每下载成功一首，下载界面就整个重来一遍（光标回到第一首、已加载的页全丢、
需要重新等 HTTP）。

根因：`poll_archives` 在回调之后调用 `reload_scripts()`，而那是**重建整个 Lua 状态**
（`LuaProvider::new_lua()` + 重新注册库），脚本里的 `songs` / `cursorPos` /
筛选状态全部归零。

修复：删掉这次 `reload_scripts()`。脚本自己的 `archive_callback` 已经写了
`downloaded[id] = "Downloaded"`，下一帧 `render` 就会把标签画出来，
本地谱面列表则由 `refresh_song_providers()` 刷新，不需要重建 Lua。

##### 下载界面的触摸

这一屏是 `touch_as_mouse()`，触摸被转成合成鼠标事件，因此：

* 屏幕面板（`IosTouchState`）**碰不到**——`route_egui_touch` 直接吞掉了触摸，
  面板上的键「点了没反应」。现在和主菜单一样 `set_auto_hidden`，整块面板和左上角
  隐藏按钮都不再绘制。
* 拖动本来只更新光标，不会翻页。

改动：

* `app.rs` 的 `set_auto_hidden` 扩展到 `"Get Songs"`。
* 皮肤 `downloadscreen.lua` 自己画一个左上角 **Back** 按钮（`RoundedRect` + 文字；
  必须画在 `draw_search` 之后，搜索条会覆盖整条顶边），
  命中后走 `exit_screen()`（存 `nautica.json` 再 `dlScreen.Exit()`），
  手柄的 `BUTTON_BCK` 复用同一个函数。
* `DownloadScreen::on_event` 现在**把按压押后到抬手**再交给脚本：
  手势位移超过 `DRAG_TAP_SLOP`（14px）就算拖动，只翻页不选中；
  否则按点击处理（选中 / 再点一次下载）。
* `tick` 里翻页的旋钮增量改成**左右两个旋钮相加**，所以转哪边都能翻。

##### 跟手翻页

现象：拖动是「每 200px 硬跳一格」，不像手机那样跟着手指走。

根因：`on_event` 自己攒位移、按固定像素折算成整数步，再把整数步交给 Lua，
所以列表只能一格一格地跳，Lua 侧完全不知道手指在哪。

修复：位移**原样转发**给脚本，几何计算交给脚本自己做（条目尺寸、列数都在那里）：

* Rust 侧 `drag_begin` / `drag_moved(dx, dy)` / `drag_released` 三个新入口，
  `DRAG_POINTS_PER_ENTRY` 和 `drag_progress` 删掉。
* 抬手时**无条件**调用 `drag_released`：点击没有位移可结算，但脚本必须知道
  手势结束了，否则下一帧还会按「正在拖动」处理。
* 脚本里 `dragOffsetX/Y` 在按下期间逐帧累加到 `gfx.Translate` 上，网格就跟着手指走；
  抬手时按 `entryW/entryH` 四舍五入成整数条目（`cursorPos`），**余数留在 offset 里**，
  由 `render` 以 `deltaTime*12` 衰减到 0，于是列表是「滑到位」而不是「跳过去」。
* 快速甩动额外多走最多 2 格（`dragLastX/Y / 60`），像手机的惯性；
  上下不会拖出列表范围（`drag_moved` 里按 `yOffset` 夹住）。

##### 等级 / 排序按钮

现象：皮肤里 `screenState==1`（等级筛选）和 `==2`（排序）的实现一直是完整的，
但只有手柄的 `BUTTON_FXL` / `BUTTON_FXR` 能进，触摸屏上没有入口。

修复：把原来只画提示文字的 `render_hotkeys` 换成底部两个**可点按钮**
（`filter_button_rect` / `sort_button_rect`，在 `resY-50-bottomButtonH-10`，
避开左下角 Nautica 角和右下角 LOADING 角），标签实时显示当前值
（`Levels: All` / `Levels: 3,15` / `Sort: Uploaded`）。

* `mouse_pressed` 先判按钮（`screen_button_pressed`），开着面板时再点一次就关掉。
* 面板里的条目在屏幕中线上，所以命中要求 `math.abs(mx - resX/2) <= 200`；
  点在别处是关闭面板，不再顺手重发一次请求。
* 旋钮在面板里改的是 `levelcursor` / `sortingcursor`（`advance_selection` 已有）。

##### 本地列表刷新

现象：切到 `Local Files` 时列表不刷新（空列表 / 还是上一次的内容）。

根因有两处：

1. `FileSongProvider::get_all` 返回的是 `all_songs` 缓存 + 数据库查出来的 `order`，
   而缓存是 `update()` 填的 —— `update()` 只在游戏主循环里跑。iOS 是**直接进选曲界面**的
   （启动即 `SongSelectScene::new`），此时缓存还是空的，于是 `order` 有 3 条、歌一首没有；
   之后 worker 只会广播「没发过」的歌（`update` 去重），界面再也补不回来。
2. 列表是按 provider 缓存建的，而 Get Songs 解包完谱面只是**请求重新扫描**，
   导入在 worker 上异步跑，进来时可能还差一次扫描。

修复：

* `get_all` 在缓存为空时**直接读数据库**（`songs_from_db`，和 `load_db` 共用），
  不再把空列表交给界面。
* `SongSelectScene::new` 在来源是 `Files` 时 `refresh()` 一次，进来看到的
  就是文件夹里真实的内容。
* `SongCollection::append` 不再无条件 `order.push`：`add` 已经用数据库的
  `order` 铺好了，provider 再报一次同一首会被列出两遍。
* 新增日志 `Song select: <provider> provider, N songs, M in the order`，
  下次日志能直接区分「查询为空」和「界面没画」。

### 4.7 配置持久化

| 提交 | 说明 |
| --- | --- |
| `3818dbc` | 全新安装时保存配置不再失败 |
| `2699225` | 换 iOS 手柄绑定的持久化 key，丢弃历史坏数据 |
| `0e61ea9` / `6d5a07f` | 新增旋钮灵敏度、旋钮速度设置 |
| `5f5f473` | 手柄绑定写回 `controller_binds`，随 `config.save()` 持久化 |

### 4.8 其它

- `2c47f78`：接受旧皮肤里的 `bitcrusher` 效果名。
- `ae4c8ae`：修 nautica 的 Shift-JIS 谱面解析。
- `91a06e5`：去掉 bundle 资源拷贝（`embed-assets` 已经带上）。
- `f85dd9e`：iOS 分支不跑桌面的 installer matrix。

---

## 5. 产物与安装

- 构建产物：artifact `USC-ios-unsigned`，仓库内归档在 `build-ipa/`
  - `build-ipa/USC-unsigned.ipa`（最新）
  - `build-ipa/USC-unsigned-<sha>.ipa`（按提交归档）
- IPA **不入库**（`.gitignore` 忽略 `build-ipa/`、`*.ipa`）。
- 安装：用 LiveContainer 侧载无签名 IPA。

---

## 6. 已知问题与待办

- [x] **本地谱面无法从「开始游戏」进入**：设置对话框的 Song Provider 标签页可以切换，
      选择持久化在 `GameConfig::song_select.provider`，主菜单按它决定初始列表。
- [ ] 下载的谱面按 nautica UUID 建目录（`songs/nautica/<id>/`），不是按曲名。
- [ ] 皮肤背景 shader 的 GLSL ES 3.0 编译失败（只影响背景层）。
- [ ] 偶发 `gl error 0x500` / `egui_glow` `GL_INVALID_OPERATION 0x502`，目前无可见影响。
- [ ] `tests::serializer::ksh_parser` 是上游就存在的失败用例。
- [ ] `download_screen.rs::start_download` 仍是 `std::thread::spawn` + `reqwest::blocking`。

---

## 7. 关键文件索引

| 文件 | 作用 |
| --- | --- |
| `ios/KsonGame/Classes/KsonGameView.m` | UIKit 视图、触摸、帧驱动 |
| `ios/KsonGame/Classes/KsonGamepad.m` | `GameController` 桥、旋钮轴捕获 |
| `ios/project.yml` | XcodeGen 工程、链接参数、构建脚本 |
| `ios/gilrs-stub/` | iOS 上的 gilrs 替身 |
| `game/src/platform/app.rs` | iOS 主循环、触摸路由 |
| `game/src/platform/render.rs` | EAGL/femtovg 渲染 |
| `game/src/platform/input.rs` | 虚拟面板绘制与命中 |
| `game/src/platform/gamepad.rs` | 手柄事件队列、绑定、`drain` |
| `game/src/platform/paths.rs` | 容器路径、资源解包、谱面目录 |
| `game/src/touch.rs` | 触摸网格与虚拟按键布局 |
| `game/src/download_screen.rs` | Get Songs 场景与 `dlScreen` 绑定 |
| `game/src/game_main.rs` | 场景栈、输入分发、帧循环 |
| `game/src/egui_host.rs` | iOS 的 egui 宿主 |
| `ios/README.md` | iOS 构建与链接说明 |
