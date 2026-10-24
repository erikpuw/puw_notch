# PUW Notch

**Tiếng Việt** · [English](README.en.md)

Notch kiểu Dynamic Island cho Windows. Rust + Direct2D, exe ~523 KB, không WebView.

[![Mời tôi một ly cà phê](https://img.shields.io/badge/M%E1%BB%9Di%20t%C3%B4i%20m%E1%BB%99t%20ly%20c%C3%A0%20ph%C3%AA-PayPal-00457C?logo=paypal&logoColor=white)](https://paypal.me/erikpuw)

## Tính năng

- **Notch**: giờ, vòng pin, sóng nhạc. Rê chuột để mở bảng. Thu mini khi thanh tab đầy chữ, ẩn khi chơi game / xem phim toàn màn hình.
- **4 trang** (chuyển bằng ‹ ›):
  - **Nhạc**: ảnh bìa, lời bài hát (LRCLIB), ⏮ ⏯ ⏭, bấm để tua. Hỗ trợ mọi app dùng media Windows, iTunes, foobar2000.
  - **Việc cần làm**: thêm / tick / sửa / xóa, nhắc theo giờ 24h ("Họp nhóm 20:00").
  - **Lịch hẹn**: "Khám răng 28/10 14:30", nhắc trước 15 phút và đúng giờ.
  - **Sức khỏe**: nhắc uống nước và đứng dậy, chỉnh được giờ và mức.
- **Banner**: thông báo Windows, âm lượng, độ sáng, mic, Caps/Num Lock, Wi-Fi, Bluetooth, mạng, pin.
- **Tray**: Notch tối giản, Khởi động cùng Windows, Ngôn ngữ (Tiếng Việt / English), Thoát.

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

Test: `cargo test`. Cài đặt lưu ở `%APPDATA%\PuwNotch\`.

Thông báo đọc từ trung tâm thông báo Windows, nên **thông báo Windows phải đang bật**. Muốn chỉ thấy trên notch thì tắt "Hiện banner" cho từng app.

## Ủng hộ

Nếu PUW Notch có ích, mời tôi một ly cà phê ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
