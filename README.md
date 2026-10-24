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
- **Thường** (150×30): giờ (chữ đậm 15 px). **Hover** 80 ms thì mở bảng (440×180).
- **Pin** (laptop): vòng tròn mức pin cạnh giờ — xanh lá + tia sét khi đang sạc, đỏ khi ≤ 20%, trắng khi bình thường.
  PC không có pin thì không hiện gì. Khi có nhạc vòng pin nằm trước cột sóng.
- **Mini** (44×4): khi có nhiều chữ ở **cả hai bên** notch (thanh nhiều tab), đo 2 lần liên tiếp và chỉ khi cửa sổ
  đứng yên (phóng to / thu nhỏ / mở lại không làm nó tụt mini), hoặc bật "Notch tối giản".
  Ở mini chỉ chính vạch 44×4 nhận chuột.
- **Ẩn khi fullscreen** (game, video) trên màn hình có notch; desktop/taskbar không tính. Không hiện banner khi đang ẩn.

**Bảng mở rộng** (4 trang: Nhạc → Việc cần làm → Lịch hẹn → Sức khỏe; mũi tên ‹ › hai bên, chấm trang ở dưới;
chuyển trang có hiệu ứng morph: trang cũ trượt ra mờ dần, trang mới trượt vào nảy nhẹ)
- Dòng đầu: icon + tên trang bên trái (♪ Nhạc, ☑ Việc cần làm, 📅 Lịch hẹn, ♥ Sức khỏe; morph theo trang),
  "CN, 25/10 · 01:32" và pin bên phải.
- Trang **Nhạc**: ảnh bìa, tên bài, ca sĩ, câu lời đang hát, `00:33 ━━ -03:26` (bấm để tua), ⏮ ⏯ ⏭.
- Trang **Việc cần làm**: ô "Thêm việc…" (bấm để gõ, Enter lưu và gõ tiếp, Esc hủy), tick vòng tròn để xong
  (chữ gạch ngang, xuống cuối), ✎ sửa / ✕ xóa (hiện khi rê chuột), "Xong hôm nay", lăn chuột để cuộn khi > 3 việc.
- **Nhắc giờ (24h)**: gõ giờ trong nội dung ("Họp nhóm 20:00", "8h30 gửi báo cáo", "17h"). Giờ còn tới hôm nay thì nhắc
  hôm nay, đã qua thì ngày mai ("Mai 08:30"). Nhãn giờ xám / cam (≤ 15 phút) / đỏ (quá giờ).
- Trang **Lịch hẹn**: "Khám răng 28/10 14:30", "Họp mai 9h", "Gọi điện 16:00" (24h; không giờ = 08:00). Nhắc trước
  15 phút và đúng giờ. Cột ngày "Hôm nay / Mai / dd/mm", sửa (bấm dòng hoặc ✎), xóa ✕. Ngày đã qua tự dọn.
- Trang **Sức khỏe**: thẻ **Uống nước** (đã uống / mục tiêu, nút "+ 250 ml", "Để sau" 10 phút; nhắc chia đều trong
  khung giờ 08:00–17:00, đủ mục tiêu thì báo và ngừng) và thẻ **Đứng dậy** (phút ngồi liên tục, nhắc mỗi 45 phút,
  rời máy ≥ 5 phút thì tính lại, nút "Đã đứng dậy"). ⚙ mở cài đặt: công tắc Nhắc, −/+ cho mỗi lần, mục tiêu,
  từ / đến, số phút. Không nhắc khi đang chơi game fullscreen.
- Banner nhắc (việc, lịch hẹn, uống nước, đứng dậy) chuyển sẵn bảng sang trang tương ứng.
- Icon đổi có hiệu ứng morph (⏯, ⚙ ↔ ✓, icon banner): icon cũ thu lại, icon mới bật ra.
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
- Các nút ⏮ ⏯ ⏭ chỉ hiện icon, không có nền tròn; rê chuột đổi màu và hiện con trỏ tay. Nút trình phát không hỗ trợ sẽ mờ và không nhận bấm.
- Bấm thanh tiến độ để tua khi trình phát cho phép. Thời gian tính theo điểm bắt đầu timeline, mốc cập nhật và tốc độ phát; không tự chạy đến cuối khi ứng dụng gửi mốc giờ rỗng.
- Tải ảnh bìa/lời chạy riêng, không giữ luồng nhận lệnh phát/dừng/chuyển bài. Lệnh gửi đúng phiên media đang hiển thị; nếu trình phát từ chối sẽ có dòng báo trong bảng.
- Ảnh bìa tải xong sẽ được nạp vào bộ vẽ ngay cả khi tên bài không đổi. Khi trình phát không gửi timeline, bảng ghi rõ "Trình phát chưa cung cấp thời gian" thay vì tự dựng thời gian.
- foobar2000: bổ sung thời gian thật từ thanh trạng thái của giao diện mặc định khi Windows không gửi timeline; lệnh phát/dừng/chuyển bài vẫn dùng phiên Windows. Chỉ bật tua nếu trình phát công bố hỗ trợ.
- iTunes: tự nhận qua COM khi ứng dụng đang mở, gồm tên bài, nghệ sĩ, thời gian, ảnh bìa và điều khiển phát/dừng/chuyển bài/tua; không tự khởi động iTunes khi ứng dụng đã đóng.
- Tự ưu tiên nguồn đang phát; nếu iTunes và nguồn Windows cùng phát thì ưu tiên iTunes khi cửa sổ iTunes đang ở trước. Các trình phát khác tiếp tục nhận tự động qua Windows media.

**Icon app**: `assets/puw.ico` / `assets/puw.res` (vẽ bằng `assets/make_icon.py`), nhúng vào exe qua `build.rs`; dùng cho tray.

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
| `src/foobar.rs` | Đọc timeline thật từ status bar foobar2000 qua UI Automation, có giới hạn thời gian chờ và test parser. |
| `src/itunes.rs` | Đọc/điều khiển iTunes qua COM, lấy ảnh bìa bằng file tạm và tự dọn file. |
| `src/calendar.rs` | Lịch hẹn: đọc ngày/giờ 24h, nhắc trước 15 phút và đúng giờ, sửa/xóa, lưu `calendar.txt` (có test). |
| `src/health.rs` | Uống nước + đứng dậy: lịch nhắc, thời gian rời máy, cài đặt −/+, lưu `health.txt` (có test). |
| `src/todo.rs` | Danh sách việc: thêm, tick, xóa, dọn theo ngày, đọc/ghi file (có test). |
| `src/system.rs` | Giờ, pin (+ tiết kiệm pin) và sự kiện pin (có test), file cài đặt, khóa Run trong registry. |
| `src/main.rs` | Cửa sổ, timer, chuột, tray, nối các phần. |

## Kế hoạch

4. Pomodoro, kệ file.
