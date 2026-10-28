# PUW Notch

**Tiếng Việt** · [English](README.en.md)

Notch kiểu Dynamic Island cho Windows. Rust + Direct2D, exe ~523 KB, không WebView.

## Tính năng

- **Notch**: giờ, vòng pin, sóng nhạc. Rê chuột để mở bảng. Tự thu mini khi viên thuốc (ngắn hay dài theo nhạc) đè lên tab hoặc chữ bên dưới, hiện lại khi hết, ẩn khi chơi game / xem phim toàn màn hình.
- **4 trang** (chuyển bằng ‹ ›):
  - **Nhạc**: ảnh bìa, lời bài hát (LRCLIB), ⏮ ⏯ ⏭, bấm để tua, nút lời bài hát giãn bảng ngang ra để xem lời chạy theo bài (dòng đang hát sáng ở giữa, dòng dài tự xuống dòng), nền trang Nhạc ngả dần sang màu ảnh bìa ở phía dưới khi đang phát. Hỗ trợ mọi app dùng media Windows, iTunes, foobar2000.
  - **Việc**: một danh sách cho cả việc và lịch hẹn. Gõ "Mua sữa", "Họp 20:00" (chỉ giờ = hôm nay) hay "Khám răng 28/10 14:30", "Gọi mẹ mai 9h" (có ngày = theo lịch). Tick / sửa / xóa; quá giờ chưa xong thì giữ lại, tô đỏ. ⚙ chọn nhắc trước 0–60 phút, luôn nhắc thêm đúng giờ.
  - **Thông báo**: đọc lại ~30 thông báo Windows gần nhất (mới nhất trên cùng, lăn chuột để cuộn), kèm icon của từng app.
  - **Sức khỏe**: nhắc uống nước và đứng dậy, chỉnh được giờ và mức.
- **Banner**: thông báo Windows, âm lượng, độ sáng, mic, Caps/Num Lock, Wi-Fi, Bluetooth, mạng, pin, tiết kiệm pin, thiết bị USB cắm và rút (kèm tên thiết bị). Icon dạng nét, mỗi icon chuyển động riêng. Chụp màn hình bằng Snipping Tool (Win+Shift+S) thì banner có ảnh thu nhỏ, bấm vào để mở ảnh vừa chụp. Bấm vào banner thông báo thì mở app đã gửi (hoặc link của thông báo, ví dụ Windows Security); mở app chứ chưa nhảy tới đúng tin nhắn.
- **Robot khi để yên**: không đụng máy 30 giây thì khuôn mặt LED (mắt + miệng) thay đồng hồ, nhìn quanh phía dưới, chớp mắt và đổi biểu cảm (tò mò, nháy mắt, phấn khích; hay cười khi đang sạc; buồn khi pin yếu); 5 phút thì ngủ, chữ z pixel bay lên; làm việc lại thì cười toe nhìn theo chuột, về giữa rồi trả về đồng hồ và pin. Bật / tắt ở tray.
- **Ẩn bảng âm lượng của Windows** (tray, mặc định tắt): app tự xử lý phím tăng / giảm / tắt tiếng nên chỉ notch hiện, bảng OSD của Windows không hiện. Phím độ sáng và mic của laptop thường đi qua firmware nên chưa chặn được.
- **Tray**: Notch tối giản, Robot khi để yên, Ẩn bảng âm lượng của Windows, Khởi động cùng Windows, Ngôn ngữ (Tiếng Việt / English), Giới thiệu PUW Notch (phiên bản, mô tả, liên kết GitHub), Thoát.

## Ảnh chụp

| | |
|---|---|
| ![Nhạc](assets/screenshots/page-music.png)<br>**Nhạc**: ảnh bìa, tên bài, lời, nền ngả theo màu ảnh bìa | ![Nhạc khi không phát](assets/screenshots/page-music-empty.png)<br>**Nhạc** khi chưa phát gì |
| ![Việc](assets/screenshots/page-tasks.png)<br>**Việc**: việc và lịch hẹn trong một danh sách | ![Thông báo](assets/screenshots/page-notifications.png)<br>**Thông báo**: kèm icon của từng app |
| ![Sức khỏe](assets/screenshots/page-health.png)<br>**Sức khỏe**: nhắc uống nước, đứng dậy | ![Banner](assets/screenshots/banner.png)<br>**Banner** thông báo |

![Nhạc với lời](assets/screenshots/page-music-lyrics.png)<br>**Nhạc** với lời bài hát: bảng giãn ngang, dòng đang hát sáng ở giữa

Viên thuốc thu gọn:

| Đang phát nhạc | Bình thường | Robot khi để yên | Robot vui |
|---|---|---|---|
| ![](assets/screenshots/pill-music.png) | ![](assets/screenshots/pill-idle.png) | ![](assets/screenshots/robot.png) | ![](assets/screenshots/robot-happy.png) |

## Tải về và chạy

Tải `puw_notch.exe` ở mục [Releases](https://github.com/erikpuw/puw_notch/releases) rồi chạy. App chưa có chữ ký số nên Windows có thể chặn:

- **SmartScreen** ("Windows protected your PC"): bấm **More info** → **Run anyway**.
- **File bị đánh dấu tải từ mạng**: chuột phải vào file → **Properties** → tick **Unblock** → OK.
- **Smart App Control** (Windows 11, hiện "App blocked" và không có nút chạy tiếp): tắt ở Settings → Privacy & security → Windows Security → App & browser control → Smart App Control settings → **Off**. Tắt rồi không bật lại được nếu không cài lại Windows, nên cân nhắc. Cách khác là tự build từ mã nguồn (mục dưới), bản build trên máy mình thường chạy được.

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

Test: `cargo test`. Cài đặt lưu ở `%APPDATA%\PuwNotch\`.

Thông báo đọc từ trung tâm thông báo Windows, nên **thông báo Windows phải đang bật**. Muốn chỉ thấy thông báo trên notch, không hiện banner ở góc phải màn hình: bật **Không làm phiền** của Windows (Cài đặt → Hệ thống → Thông báo). Windows vẫn giữ thông báo trong trung tâm thông báo nên notch vẫn đọc và hiện banner như bình thường. Windows chưa cho app khác tự tắt banner của từng app đang chạy, nên việc này do bạn bật.

## Ủng hộ

Nếu PUW Notch có ích, mời tôi một ly cà phê ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
