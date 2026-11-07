<p align="center">
  <img src="assets/puw_1024.png" width="112" alt="Biểu tượng PUW Notch">
</p>

<h1 align="center">PUW Notch</h1>

<p align="center"><b>Chiếc notch sống ngay trên Windows của bạn.</b></p>

<p align="center">
  Nhạc, việc, thông báo, thời tiết, lịch và một chú robot nhỏ, gói gọn ở mép trên màn hình theo kiểu Dynamic Island.<br>
  Rust + Direct2D vẽ trên GPU qua DirectComposition, exe ~794 KB, không WebView.
</p>

<p align="center"><b>Tiếng Việt</b> · <a href="README.en.md">English</a></p>

> **Chỉ tải từ nguồn của tác giả.** PUW Notch do tác giả (erikpuw) phát hành duy nhất tại [github.com/erikpuw/puw_notch](https://github.com/erikpuw/puw_notch). Bản tải từ nơi khác (trang chia sẻ, kho khác, bản đóng gói lại, tệp gửi qua mạng) không do tác giả kiểm soát và không được đảm bảo: có thể đã bị sửa đổi hoặc chèn thêm mã. Hãy chỉ dùng bản ở mục [Releases](https://github.com/erikpuw/puw_notch/releases) của kho này.

## Tính năng

| Tính năng | Làm gì |
|---|---|
| **Notch** | Giờ, vòng pin (xanh khi sạc, vàng khi tiết kiệm pin, đỏ từ 20%) và sóng nhạc. Rê chuột để mở bảng. Hai kiểu, chọn ở tray: tai thỏ hoặc viên thuốc nổi. |
| **Tự né** | Thu mini khi đè lên chữ bên dưới (tab của Edge, Terminal), ẩn khi chơi game hoặc xem phim toàn màn hình. Nó hỏi thẳng cửa sổ phía dưới thay vì chụp màn hình, nên vẫn hiện khi quay, chia sẻ màn hình hay điều khiển từ xa. |
| **Banner** | Thông báo Windows (icon của app, giờ nhận, nút × tắt sớm, ảnh thu nhỏ cho ảnh chụp màn hình), âm lượng, độ sáng, mic, Caps/Num Lock, pin, tiết kiệm pin, USB, Wi-Fi, Bluetooth, máy bay. Nhiều thông báo đến cùng lúc thì hiện lần lượt theo thứ tự đến. |
| **Banner mạng** | Wi-Fi, Bluetooth và máy bay mỗi thứ một banner riêng; máy bay gom các thứ đi kèm. Khi Wi-Fi bật hoặc máy bay tắt, banner là danh sách các bước (kết nối mạng, VPN đã chạy trước đó, Bluetooth và thiết bị đã nối): tích xanh khi xong, vòng quay khi đang chờ, vòng rỗng khi chưa tới lượt, dấu chéo đỏ khi quá 15 giây không được. Một thiết bị Bluetooth là banner riêng có pin lớn, từ hai thiết bị là danh sách có pin nhỏ. Bật tắt nhanh chỉ để lại trạng thái cuối. |
| **Tác vụ trực tiếp** | Banner tiến trình cho sao chép, di chuyển, xóa tệp trong Explorer, tải file và lệnh build trong terminal. Mặc định tắt, [chi tiết](#tác-vụ-nền-trực-tiếp). |
| **Duyệt trên notch** | Câu hỏi của Claude Code, Codex và Antigravity (xin quyền, chọn đáp án) hiện thành banner; bấm **Đồng ý** / **Không** hoặc chọn đáp án. Không tự duyệt. Banner đóng sau 20 giây (app hỏi lại như thường) hoặc ngay khi bạn trả lời trong app. [Cách thiết lập](#thiết-lập-cho-claude-code-tuỳ-chọn). |
| **Robot** | 30 giây không đụng máy thì mặt LED (chỉ có đôi mắt) thay đồng hồ: nhìn quanh, mừng khi pin vừa đầy, buồn khi pin từ 20% trở xuống mà chưa sạc. Động chuột thì liếc theo chuột; 5 phút thì ngủ. Bật / tắt ở tray. |
| **Nhẹ** | Chỉ tải phần đang hiện, ẩn toàn màn hình thì ngừng tải. Khi nghỉ chừng 7 MB RAM và 0% CPU, phát nhạc dạng pill chừng 14 MB, mở trang Nhạc có aurora chừng 22 MB ([ảnh Task Manager](#nhẹ-và-menu-tray)). |
| **Tray** | Notch tối giản, Robot, Kiểu notch, Trang hiển thị, Tính năng khác (Duyệt ngay trên notch, Tác vụ nền trực tiếp, Ẩn bảng âm lượng Windows), Khởi động cùng Windows, Ngôn ngữ (Tiếng Việt / English), Giới thiệu, Thoát. |

Trang ẩn ở tray thì ngừng hẳn: không nhắc việc, uống nước hay đứng dậy, không đếm Pomodoro, không đọc nhạc, thời tiết, hạn mức hay thông báo.

**Khởi động cùng Windows** hỏi quyền quản trị một lần để tạo tác vụ `PuwNotch` trong Task Scheduler. Từ đó notch tự chạy với quyền quản trị 15 giây sau khi đăng nhập, không hỏi lại, và đọc được cả cửa sổ chạy admin. Bỏ tick thì tác vụ bị xóa.

**8 trang**, chuyển bằng ‹ › hoặc rê chuột vào chấm trang: chấm mở thành hàng biểu tượng để chọn thẳng một trang.

| Trang | Làm gì |
|---|---|
| **Nhạc** | Ảnh bìa, lời đồng bộ (LRCLIB), ⏮ ⏯ ⏭, trộn bài, lặp, kéo thanh để tua (Windows media controls, iTunes, foobar2000; nút nào trình phát không có thì để mờ). Nút lời mở rộng bảng; nền và đường aurora theo màu bìa. |
| **Ghi chú** | Ghi chú và lịch hẹn trong một danh sách, vòng tiến độ cho biết còn bao nhiêu. Gõ "Họp 20:00", "Khám răng 28/10 14:30" hay "Gọi mẹ mai 9h". Quá giờ tô đỏ "Trễ"; ⚙ chọn nhắc trước 0 đến 60 phút. |
| **Tập trung** | Pomodoro: cả chu kỳ (4 vòng tập trung 25 phút, nghỉ ngắn 5, nghỉ dài 15) là một thanh chia đoạn. Hết đoạn thì banner báo và đoạn kế tự chạy (sau nghỉ dài thì chờ chu kỳ mới). Khi đang chạy, pill có một vạch mỏng ở đáy. |
| **Thông báo** | ~30 thông báo Windows gần nhất, mỗi tin có icon app và hai dòng. Bấm để mở app; "Xóa hết" chỉ ẩn trong notch, Windows vẫn giữ. |
| **Sức khỏe** | Nhắc uống nước và đứng dậy: thanh tiến độ trong ngày, nút tròn để ghi một lần uống hoặc đã đứng dậy, chỉnh giờ và mức ở ⚙. |
| **Thời tiết** | Gõ tên thành phố một lần: nhiệt độ lớn, 5 mốc giờ tới, cảm giác, độ ẩm, gió, nhiệt độ cao nhất, °C \| °F. Nút mặt trời đổi dải giờ thành **đường đi của mặt trời** (giờ mọc, lặn, còn bao lâu nắng; ban đêm có trăng khuyết). Dữ liệu Open-Meteo, làm mới 30 phút. |
| **Lịch** | Một tháng, tuần xếp từ thứ Hai đến Chủ nhật (ISO 8601), ngày đang chọn hiện lớn bên trái cùng thứ. Bấm một ngày để xem, ‹ › để đổi tháng. Không có ngày lễ vì mỗi nước một khác. Ở máy đặt vùng Việt Nam, Trung Quốc, Hồng Kông, Ma Cao, Đài Loan hoặc Hàn Quốc, nút **Dương \| Âm** đổi các số trên lưới sang âm lịch (tự tính, không cần mạng). Đổi vùng trong Windows thì nút hiện hay ẩn theo. |
| **Giới hạn** | Hạn mức 5 giờ và tuần của Claude, Codex, Antigravity (gói free: 30 ngày), mỗi app một tab, chỉ hiện app tìm thấy trên máy. Claude hiện phần đã dùng (hai vòng đồng tâm: 5 giờ ngoài, tuần trong); Codex và Antigravity hiện phần còn lại. Banner báo khi qua 80%, 95% hoặc dùng từ 25% trong một ngày. |

Trang Giới hạn chỉ đọc trên máy, không gửi đi đâu:

| App | Lấy từ |
|---|---|
| **Claude** | `plan-usage-history.json` do app desktop tự ghi (tìm trong `%APPDATA%\Claude`, `Claude-Data` và thư mục gói của app; số cũ vẫn hiện kèm tuổi, mở khung giới hạn trong app để làm mới). Claude Code ở terminal qua `statusLine`. |
| **Codex** | Log phiên `~/.codex`, cập nhật sau mỗi lượt agent. Số quá 10 phút ghi "Số cũ"; đổi tài khoản thì chờ lượt chạy đầu. |
| **Antigravity** | Máy chủ cục bộ của chính app, chỉ khi app đang mở. |

Ghi công: dữ liệu thời tiết [Open-Meteo](https://open-meteo.com) (CC BY 4.0), icon thời tiết và pin [Lucide](https://lucide.dev) (ISC), icon banner [Tabler Icons](https://tabler.io/icons) (MIT).

## Tác vụ nền trực tiếp

Bật ở **tray → Tính năng khác → Tác vụ nền trực tiếp** (mặc định tắt, lựa chọn được lưu). Việc quét chạy trên luồng nền, không chặn luồng vẽ.

- **Explorer:** đọc cửa sổ tiến trình sao chép, di chuyển, xóa và giải nén. Banner có tên thao tác, phần trăm (đọc từ tiêu đề cửa sổ) và thanh tiến trình. Không bắt được thao tác quá nhanh, không mở cửa sổ tiến trình, hoặc công cụ quản lý file khác.
- **Tải file:** theo dõi file tạm `.crdownload`, `.part`, `.opdownload` trong thư mục Downloads do Windows cấu hình và thư mục tải riêng của Edge, Chrome, Brave (mọi profile). Banner ghi tên file, MB đã tải và MB/s. Chromium đặt tên tạm "Chưa xác nhận 12345" thì tên thật đọc từ lịch sử tải của trình duyệt (chỉ đọc; InPrivate không có lịch sử nên chỉ hiện "Đang tải xuống"). Tải xong banner chuyển thành "Tải xuống hoàn tất" với dấu tích xanh trong 5 giây, bấm vào để mở thư mục với file đã chọn sẵn. Không có phần trăm vì file tạm không cho biết tổng dung lượng. Chưa hỗ trợ Firefox hay trình duyệt hỏi nơi lưu từng file.
- **Build:** cargo, msbuild, dotnet, make, cmake, ninja, go chạy từ terminal. Kết thúc báo hoàn tất (dấu tích xanh) hoặc thất bại (dấu ✕ đỏ) theo mã thoát, nếu notch mở được tiến trình đó; không thì chỉ có "Đã kết thúc".
- **Hiển thị:** mỗi lần một tác vụ, ưu tiên Explorer, rồi tải file, rồi build. Còn tác vụ khác thì tiêu đề có thêm **+N**; tác vụ bị che vẫn được theo dõi và báo khi xong. Banner nhường thông báo khác và câu hỏi duyệt.

## Ảnh chụp

| Ảnh chụp | Ảnh chụp |
|---|---|
| ![Nhạc](assets/screenshots/page-music.png)<br>**Nhạc**: ảnh bìa, tên bài, lời, nền và đường aurora theo màu bìa | ![Nhạc khi không phát](assets/screenshots/page-music-empty.png)<br>**Nhạc** khi chưa phát gì |
| ![Ghi chú](assets/screenshots/page-tasks.png)<br>**Ghi chú**: ghi chú và lịch hẹn trong một danh sách | ![Thông báo](assets/screenshots/page-notifications.png)<br>**Thông báo**: kèm icon của từng app |
| ![Tập trung](assets/screenshots/page-focus.png)<br>**Tập trung**: cả chu kỳ Pomodoro trên một thanh | ![Pill đang đếm giờ](assets/screenshots/pill-focus.png)<br>**Pill** khi đang đếm: một vạch mỏng ở đáy |
| ![Sức khỏe](assets/screenshots/page-health.png)<br>**Sức khỏe**: nhắc uống nước, đứng dậy | ![Banner](assets/screenshots/banner.png)<br>**Banner** thông báo |

![Nhạc với lời](assets/screenshots/page-music-lyrics.png)<br>**Nhạc** với lời bài hát: bảng giãn ngang, dòng đang hát sáng ở giữa, đường aurora lượn dọc đáy bảng

![Chọn trang nhanh](assets/screenshots/page-nav.png)<br>**Chọn trang nhanh**: rê chuột vào chấm trang, chấm mở thành hàng biểu tượng, bấm một biểu tượng để sang thẳng trang đó

| ![Thời tiết](assets/screenshots/page-weather.png)<br>**Thời tiết**: icon lớn, nhiệt độ, 5 mốc giờ tới, nút °C / °F | ![Đường đi của mặt trời ban ngày](assets/screenshots/page-weather-sun.png)<br>**Ban ngày**: cung xanh từ lúc mọc đến lặn, mặt trời đứng đúng giờ hiện tại | ![Đường đi của mặt trời ban đêm](assets/screenshots/page-weather-night.png)<br>**Ban đêm**: trăng khuyết đi dọc cung từ lúc lặn đến lúc mọc |
|---|---|---|

| ![Lịch](assets/screenshots/page-calendar.png)<br>**Lịch**: ngày lớn, tuần từ thứ Hai, ‹ › đổi tháng | ![Lịch âm](assets/screenshots/page-calendar-lunar.png)<br>**Âm lịch**: nút Dương \| Âm đổi số trên lưới (chỉ ở vùng có âm lịch) |
|---|---|

| ![Bật Wi-Fi](assets/screenshots/banner-wifi.png)<br>**Bật Wi-Fi**: kết nối mạng, rồi VPN đã chạy trước đó | ![Tắt chế độ máy bay](assets/screenshots/banner-airplane-off.png)<br>**Tắt chế độ máy bay**: các bước hiện sẵn, tích dần khi xong |
|---|---|
| ![Bật chế độ máy bay](assets/screenshots/banner-airplane-on.png)<br>**Bật chế độ máy bay**: Wi-Fi, Bluetooth, VPN tắt theo | ![Hai thiết bị Bluetooth](assets/screenshots/banner-bluetooth.png)<br>**Bluetooth**: hai thiết bị trở lên, mỗi thiết bị một dòng kèm pin |

**Giới hạn** của Claude, Codex và Antigravity. Tab Codex ghi gói của tài khoản đang dùng, gói free tính theo 30 ngày:

| Claude | Codex (gói free, 30 ngày) | Antigravity |
|---|---|---|
| ![](assets/screenshots/page-usage-claude.png) | ![](assets/screenshots/page-usage-codex.png) | ![](assets/screenshots/page-usage-antigravity.png) |

**Nhắc uống nước và đứng dậy**: banner chỉ báo (nút × để đóng). Để ghi nhận thì bấm nút tròn ở trang Sức khỏe (có sẵn "Để sau 10 phút"):

| Đến giờ uống | Đạt mục tiêu | Ngồi lâu |
|---|---|---|
| ![](assets/screenshots/banner-water.png) | ![](assets/screenshots/banner-water-goal.png) | ![](assets/screenshots/banner-standup.png) |

**Duyệt ngay trên notch**: bấm là trả lời.

| Claude (câu hỏi chọn) | Antigravity (xin quyền dùng công cụ) |
|---|---|
| ![](assets/screenshots/banner-approve-claude.png) | ![](assets/screenshots/banner-approve-antigravity.png) |

**Kiểu viên thuốc** (tray, Kiểu notch): nổi cách mép trên màn hình, bo tròn đủ bốn góc. Mặc định vẫn là tai thỏ.

| Thu gọn | Đang phát nhạc | Trang mở rộng |
|---|---|---|
| ![](assets/screenshots/style-pill-idle.png) | ![](assets/screenshots/style-pill-music.png) | ![](assets/screenshots/style-pill-page.png) |

Viên thuốc thu gọn:

| Đang phát nhạc | Bình thường | Robot khi để yên | Robot vui |
|---|---|---|---|
| ![](assets/screenshots/pill-music.png) | ![](assets/screenshots/pill-idle.png) | ![](assets/screenshots/robot.png) | ![](assets/screenshots/robot-happy.png) |

Có thời tiết:

| Bình thường | Đang phát nhạc |
|---|---|
| ![](assets/screenshots/weather-pill.png) | ![](assets/screenshots/weather-pill-music.png) |

## Nhẹ và menu tray

Task Manager ở ba trạng thái (đo trên một máy, số thay đổi theo máy):

| Notch đang nghỉ | Phát nhạc, dạng pill | Mở trang Nhạc, có aurora |
|---|---|---|
| ![](assets/screenshots/task-manager.png)<br>0% CPU, 7,3 MB RAM | ![](assets/screenshots/task-manager-music-pill.png)<br>0,4% CPU, 14,5 MB RAM | ![](assets/screenshots/task-manager-music-page.png)<br>0,5% CPU, 21,6 MB RAM |

Menu ở khay hệ thống (chuột phải vào biểu tượng):

| Tiếng Việt | English |
|---|---|
| ![](assets/screenshots/tray-vi.png) | ![](assets/screenshots/tray-en.png) |

## Tải về và chạy

Tải `puw_notch.exe` ở mục [Releases](https://github.com/erikpuw/puw_notch/releases) rồi chạy (chỉ tải ở đây, xem lưu ý ở đầu trang). App chưa có chữ ký số, nên không có gì khác ngoài nguồn này để xác nhận file là của tác giả, và Windows có thể chặn:

- **SmartScreen** ("Windows protected your PC"): bấm **More info** → **Run anyway**.
- **File bị đánh dấu tải từ mạng**: chuột phải vào file → **Properties** → tick **Unblock** → OK.
- **Smart App Control** (Windows 11, hiện "App blocked" và không có nút chạy tiếp): tắt ở Settings → Privacy & security → Windows Security → App & browser control → Smart App Control settings → **Off**. Tắt rồi không bật lại được nếu không cài lại Windows, nên cân nhắc. Cách khác là tự build từ mã nguồn (mục dưới), bản build trên máy mình thường chạy được.

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

Bản release dùng `opt-level = "z"` (exe ~794 KB thay vì ~810 KB, thời gian vẽ mỗi khung hình không đổi). `build.rs` gắn icon và thông tin phiên bản (lấy từ `Cargo.toml`) vào exe, xem ở Properties → Details. Test: `cargo test`. Cài đặt lưu ở `%APPDATA%\PuwNotch\`. App ghi lúc khởi động, thoát, lỗi, tự ẩn, panel mở/đóng, khung hình trễ và GPU đang dùng vào `%APPDATA%\PuwNotch\notch.log` (tối đa 64 KB, bắt đầu lại khi đổi phiên bản), để biết vì sao app không chạy khi khởi động cùng Windows.

Thông báo đọc từ trung tâm thông báo Windows, nên **thông báo Windows phải đang bật**. Muốn chỉ thấy thông báo trên notch, không hiện banner ở góc phải màn hình, hãy bật **Không làm phiền** (Cài đặt → Hệ thống → Thông báo). Windows vẫn giữ thông báo trong trung tâm nên notch vẫn đọc và hiện banner như thường. Windows chưa cho app khác tắt banner của từng app, nên việc này do bạn bật.

## Thiết lập cho Claude Code (tuỳ chọn)

Phần duyệt ngay trên notch và số giới hạn của Claude Code ở terminal cần hai script nhỏ trong `tools/`. Chép chúng vào `~/.claude/` rồi thêm vào `~/.claude/settings.json` (thay `<tên>` bằng tên người dùng Windows):

```json
"statusLine": { "type": "command", "command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:/Users/<tên>/.claude/puw-notch-statusline.ps1" },
"hooks": {
  "PermissionRequest": [
    { "matcher": "*", "hooks": [ { "type": "command", "command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:/Users/<tên>/.claude/puw-notch-approve.ps1", "timeout": 60 } ] }
  ]
}
```

- `puw-notch-approve.ps1` (hook): gửi yêu cầu duyệt tới notch qua ống cục bộ. Notch không chạy, không bấm trong 50 giây hoặc bấm **Trong app** thì Claude hỏi như bình thường. Không có tự duyệt.
- `puw-notch-statusline.ps1` (statusLine): chỉ cần nếu bạn dùng Claude Code ở terminal; app Claude desktop thì notch đọc thẳng file của app.
- Antigravity không cần thiết lập: notch đọc từ máy chủ cục bộ của nó (khi app mở). Codex: hạn mức không cần thiết lập, duyệt lệnh xem phần dưới.

## Thiết lập cho Codex (tuỳ chọn)

Hạn mức của Codex notch đọc thẳng từ `~/.codex`, không cần thiết lập. Để **duyệt lệnh của Codex ngay trên notch**, dùng chung script `puw-notch-approve.ps1` với Claude Code: chép nó vào `~/.claude/` (hoặc chỗ bạn thích) rồi thêm một mục `PermissionRequest` vào `~/.codex/hooks.json` (thay `<tên>` bằng tên người dùng Windows):

```json
{
  "hooks": {
    "PermissionRequest": [
      { "hooks": [ { "type": "command", "command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:/Users/<tên>/.claude/puw-notch-approve.ps1 -App Codex" } ] }
    ]
  }
}
```

- Nếu `hooks.json` đã có mục `PermissionRequest`, thêm nhóm trên vào **cuối** mảng để không đổi vị trí các hook cũ (Codex lưu mã tin cậy theo vị trí).
- Codex chỉ chạy hook đã được tin cậy: mở Codex, gõ `/hooks` và tin cậy hook mới.
- Từ đó khi Codex xin quyền, banner **Codex** hiện trên notch với lệnh cần chạy; bấm **Đồng ý** hoặc **Không**. Notch không chạy hoặc không bấm trong 50 giây thì Codex hỏi như bình thường.
- `-App` là tên hiện trên banner. `-Log` (thêm vào cuối lệnh) ghi những gì hook nhận vào `%TEMP%\puw-notch-hook.log`, chỉ để xem một app gửi gì; đừng để bật thường xuyên vì nó lưu cả lệnh.

## Ủng hộ

Nếu PUW Notch có ích, mời tôi một ly cà phê ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
