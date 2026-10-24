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
- Dính mép trên màn hình chính, viền "tai thỏ" kiểu iPhone (hai góc trên loe cong, to dần theo chiều cao), co giãn kiểu lò xo.
- **Thường** (150×30): giờ. **Hover** 200 ms thì bung (440×200).
- **Mini** (44×4): khi cửa sổ phía dưới có nội dung lởm chởm sát notch (nhiều tab), hoặc bật "Notch tối giản".
  Ở mini chỉ chính vạch 44×4 nhận chuột.
- **Ẩn khi fullscreen** (game, video) trên màn hình có notch; desktop/taskbar không tính. Không hiện banner khi đang ẩn.

**Bảng mở rộng**
- Giờ lớn, thứ/ngày tiếng Việt, pin.
- Thẻ thông báo Windows mới nhất.
- Điều khiển: loa (bật/tắt tiếng), − / thanh âm lượng (bấm để đặt mức) / +, mic (bật/tắt), Bluetooth (bật/tắt radio).

**Banner tự giãn ra**
- Thông báo Windows mới: 5 giây (tên app, tiêu đề, nội dung). Để chuột lên banner thì mở bảng.
- Phím âm lượng / tắt tiếng: "Âm lượng 72%" + thanh mức.
- Độ sáng màn hình laptop thay đổi: "Độ sáng 90%" + thanh mức.
- Caps Lock / Num Lock: Bật / Tắt.

**Nhạc** (mọi app báo lên điều khiển media của Windows: Spotify, Chrome/Edge, Media Player, foobar2000...)
- Notch thường khi có nhạc: ảnh bìa bên trái, giờ ở giữa, cột sóng nhạc bên phải.
- Bảng mở rộng có thẻ nhạc: ảnh bìa, tên bài, ca sĩ, câu lời đang hát (LRCLIB), thanh tiến độ `00:33 / -03:26`, ⏮ ⏯ ⏭.
- App không gửi vị trí bài (ví dụ foobar2000 mặc định) thì không có thanh tiến độ và lời không chạy theo được.

**Khác**: tray (Notch tối giản, Khởi động cùng Windows, Thoát), chạy một bản duy nhất, cài đặt ở `%APPDATA%\PuwNotch\settings.txt`.

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
| `src/screen.rs` | Mỗi 500 ms: fullscreen, hoặc đếm điểm ảnh "cạnh" ở hai dải 60 px cạnh notch (≥12 = bận). |
| `src/render.rs` | Direct2D vẽ vào DIB 32-bit, `UpdateLayeredWindow` per-pixel alpha; vị trí các nút. |
| `src/notify.rs` | Đọc thông báo qua winsqlite3 (nạp động), tách XML toast, tên app. Có test với SQLite thật. |
| `src/audio.rs` | Âm lượng, tắt tiếng loa, mic (Core Audio, thiết bị mặc định). |
| `src/bluetooth.rs` | Radio Bluetooth (Windows.Devices.Radios) trên luồng phụ. |
| `src/status.rs` | Hook bàn phím cấp thấp (chỉ phím khóa + âm lượng), độ sáng qua WMI. |
| `src/music.rs` | Luồng phụ đọc phiên media (GlobalSystemMediaTransportControls) mỗi 500 ms, lệnh ⏮⏯⏭, tải lời LRCLIB (WinRT HttpClient + JSON), tách LRC. |
| `src/system.rs` | Giờ, pin, file cài đặt, khóa Run trong registry. |
| `src/main.rs` | Cửa sổ, timer, chuột, tray, nối các phần. |

## Kế hoạch

4. Pomodoro, nhắc uống nước, to-do, kệ file.
