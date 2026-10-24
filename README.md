# Nusa Rebuild

Notch "sống" cho Windows, viết lại từ đầu bằng Rust (Win32 + Direct2D, không WebView). Exe khoảng 160 KB.

## Build & chạy

```bash
cargo build --release
target/release/nusa_rebuild.exe
```

Test logic: `cargo test`

## Đã có (giai đoạn 1: khung notch)

- Notch đen dính mép trên màn hình chính, hiệu ứng co giãn kiểu lò xo.
- **Thường** (150×30): hiện giờ. **Hover** 200 ms thì bung (420×140): giờ lớn, thứ/ngày tiếng Việt, pin.
- **Mini** (44×4): tự về mini khi cửa sổ phía dưới có nội dung lởm chởm sát notch (nhiều tab trình duyệt/terminal),
  hoặc khi bật "Notch tối giản". Ở mini chỉ chính vạch 44×4 nhận chuột.
- **Ẩn khi fullscreen** (game, video) trên màn hình có notch; desktop/taskbar không bị tính.
- Tray: Notch tối giản, Khởi động cùng Windows, Thoát. Chạy một bản duy nhất.
- Cài đặt: `%APPDATA%\NusaRebuild\settings.txt`.

## Cách hoạt động

| File | Vai trò |
|---|---|
| `src/notch.rs` | Máy trạng thái (Collapsed/Mini/Peek/PeekMini/Expanded) + lò xo. Thuần logic, có test. |
| `src/screen.rs` | Kiểm tra mỗi 500 ms: fullscreen, hoặc đếm điểm ảnh "cạnh" ở hai dải 60 px cạnh notch (≥12 = bận). |
| `src/render.rs` | Direct2D vẽ vào DIB 32-bit, `UpdateLayeredWindow` per-pixel alpha. Điểm ảnh alpha 0 cho chuột xuyên qua. |
| `src/system.rs` | Giờ, pin, file cài đặt, khóa Run trong registry. |
| `src/main.rs` | Cửa sổ, timer, chuột, tray. |

Khác bản gốc (sửa các lỗi đã gặp khi vá exe):
- Cửa sổ layered nên ảnh chụp kiểm tra (BitBlt không CAPTUREBLT) không dính chính notch.
- Bỏ qua kiểm tra khi notch đang rộng hơn 180 px (đang bung) → không còn nhảy mini rồi bung lại.
- Fullscreen so với màn hình thật của notch, loại desktop/taskbar.

## Kế hoạch

2. Thông báo Windows (cần đóng gói có danh tính để dùng `UserNotificationListener`).
3. Nhạc + lời bài hát (LRCLIB).
4. Pomodoro, nhắc uống nước, to-do, kệ file, pin/mạng/Caps Lock.
