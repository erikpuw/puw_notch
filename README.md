# PUW Notch

Notch "sống" cho Windows, viết bằng Rust (Win32 + Direct2D, không WebView). Exe khoảng 210 KB.

## Build & chạy

```bash
cargo build --release
target/release/puw_notch.exe
```

Test: `cargo test` (thêm `cargo test -- --ignored --nocapture` để gọi LRCLIB thật và liệt kê phiên media trên máy)

## Tính năng

**Notch**
- Dính mép trên màn hình chính, viền "tai thỏ" kiểu iPhone (hai góc trên loe cong, to dần theo chiều cao).
- Co giãn kiểu lò xo nhẹ (giảm chấn ~0,78): mở và thu mượt, độ nảy vượt đích dưới 2%.
- **Thường** (150×30): giờ (chữ đậm 15 px). **Hover** 200 ms thì mở bảng (440×180).
- **Pin** (laptop): vòng tròn mức pin cạnh giờ — xanh lá + tia sét khi đang sạc, đỏ khi ≤ 20%, trắng khi bình thường.
  PC không có pin thì không hiện gì. Khi có nhạc vòng pin nằm trước cột sóng.
- **Mini** (44×4): khi cửa sổ phía dưới có nội dung lởm chởm sát notch (nhiều tab), hoặc bật "Notch tối giản".
  Ở mini chỉ chính vạch 44×4 nhận chuột.
- **Ẩn khi fullscreen** (game, video) trên màn hình có notch; desktop/taskbar không tính. Không hiện banner khi đang ẩn.

**Bảng mở rộng** (chỉ nhạc; notch không điều khiển gì khác, chỉ báo khi có thứ bật/tắt)
- Dòng đầu: thứ/ngày · giờ, pin bên phải.
- Ảnh bìa, tên bài, ca sĩ, câu lời đang hát, `00:33 ━━ -03:26`, ⏮ ⏯ ⏭ ("Không có bài" khi không phát).
- Đọc thông báo: dùng trung tâm thông báo của Windows; notch chỉ báo khi có thông báo mới.

**Banner tự giãn ra**
- Thông báo Windows mới: 5 giây, banner rộng 420 px, nội dung 2 dòng (tên app, tiêu đề, nội dung). Để chuột lên banner thì mở bảng.
- Phím âm lượng / tắt tiếng: "Âm lượng 72%" + thanh mức.
- Độ sáng màn hình laptop thay đổi: "Độ sáng 90%" + thanh mức.
- Caps Lock / Num Lock: Bật / Tắt.
- Mic bật / tắt từ bất kỳ đâu (nút trên notch, phím mic laptop, Cài đặt Windows, app gọi): "Đã tắt mic" / "Đã bật mic".
- Wi-Fi / Bluetooth / mạng di động bật-tắt, chế độ máy bay, mạng: "Đã kết nối <tên Wi-Fi>", "Không có internet",
  "Mất kết nối mạng". Bật chế độ máy bay chỉ báo một lần (không kèm Wi-Fi tắt, mất mạng, thiết bị ngắt).
- Pin: cắm / rút sạc, tiết kiệm pin bật-tắt (vòng pin chuyển vàng), pin yếu 20%, rất yếu 10%, sạc đầy.
- Thiết bị Bluetooth kết nối / ngắt: "Bluetooth · Tai nghe … · Đã kết nối" (kiểm tra mỗi 2 giây, cả Bluetooth thường và BLE),
  icon theo loại: tai nghe, loa, điện thoại, chuột, bàn phím, tay cầm (theo class Bluetooth, thiếu thì đoán theo tên).
- Mỗi banner có icon bên trái (chuông cho thông báo app, loa, độ sáng, bàn phím, thiết bị).

**Nhạc** (mọi app báo lên điều khiển media của Windows: Spotify, Chrome/Edge, Media Player, foobar2000...)
- Notch thường khi có nhạc: ảnh bìa bên trái, giờ ở giữa, (pin), cột sóng nhạc bên phải.
- Lời bài hát lấy từ LRCLIB, chạy theo vị trí bài.
- App không gửi vị trí bài (ví dụ foobar2000 mặc định) thì không có thanh tiến độ và lời không chạy theo được.

**Khác**: tray (Notch tối giản, Khởi động cùng Windows, Thoát), chạy một bản duy nhất, cài đặt ở `%APPDATA%\PuwNotch\settings.txt`.
Chọn **Thoát** sẽ kết thúc tiến trình và gỡ biểu tượng tray.

### Lưu ý về thông báo

Đọc từ cơ sở dữ liệu trung tâm thông báo của Windows (`%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db`)
bằng `winsqlite3.dll` có sẵn trong Windows, không cần đóng gói hay quyền admin.
**Thông báo Windows phải đang bật** (Cài đặt → Hệ thống → Thông báo). Muốn chỉ thấy trên notch: giữ bật,
rồi tắt "Hiện banner thông báo" cho từng app.

### Không làm được

- Đèn bàn phím: Windows không có API chung, chỉ driver riêng của từng hãng.
- Độ sáng màn hình rời: thường không báo qua WMI.

## Cấu trúc

| File | Vai trò |
|---|---|
| `src/notch.rs` | Máy trạng thái (Collapsed/Mini/Peek/PeekMini/Expanded/Alert) + lò xo. Thuần logic, có test. |
| `src/screen.rs` | Mỗi 500 ms: fullscreen, hoặc đếm điểm ảnh "cạnh" ở hai dải 60 px cạnh notch (≥12 = bận). Ảnh chụp có dính chính notch, nên chỉ lấy mẫu khi notch đang nghỉ đúng cỡ (không hover/mở/banner/co giãn). |
| `src/render.rs` | Direct2D vẽ vào DIB 32-bit, `UpdateLayeredWindow` per-pixel alpha; vị trí các nút. |
| `src/notify.rs` | Đọc thông báo qua winsqlite3 (nạp động), tách XML toast, tên app. Có test với SQLite thật. |
| `src/audio.rs` | Đọc âm lượng / tắt tiếng loa, mic (Core Audio, thiết bị mặc định). |
| `src/connectivity.rs` | Luồng phụ mỗi 2 giây: radio Wi-Fi/Bluetooth/di động, chế độ máy bay (registry), mạng (NetworkInformation), thiết bị Bluetooth; so sánh trạng thái → sự kiện (có test). |
| `src/status.rs` | Hook bàn phím cấp thấp (chỉ phím khóa + âm lượng), độ sáng qua WMI. |
| `src/music.rs` | Luồng phụ đọc phiên media (GlobalSystemMediaTransportControls) mỗi 500 ms, lệnh ⏮⏯⏭, tải lời LRCLIB (WinRT HttpClient + JSON), tách LRC. |
| `src/system.rs` | Giờ, pin (+ tiết kiệm pin) và sự kiện pin (có test), file cài đặt, khóa Run trong registry. |
| `src/main.rs` | Cửa sổ, timer, chuột, tray, nối các phần. |

## Kế hoạch

4. Pomodoro, nhắc uống nước, to-do, kệ file.
