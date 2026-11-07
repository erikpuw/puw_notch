use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Data::Json::JsonObject;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WPARAM};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APPROVE: u32 = WM_APP + 13;

pub const TIMEOUT: Duration = Duration::from_secs(50);

pub const BANNER: Duration = Duration::from_secs(20);

const MAX_REQUEST: usize = 8 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Allow,
    Deny,
    Choice(usize),
    Ask,
}

impl Answer {
    fn line(self) -> Vec<u8> {
        match self {
            Answer::Allow => b"allow\n".to_vec(),
            Answer::Deny => b"deny\n".to_vec(),
            Answer::Choice(i) => format!("choice:{i}\n").into_bytes(),
            Answer::Ask => b"ask\n".to_vec(),
        }
    }
}

pub struct Pending {
    pub id: u64,
    pub app: String,
    pub title: String,
    pub detail: String,
    pub options: Vec<String>,
    reply: Sender<Answer>,
}

impl Pending {
    pub fn answer(self, answer: Answer) {
        let _ = self.reply.send(answer);
    }
}

pub struct Server {
    queue: Arc<Mutex<VecDeque<Pending>>>,
    cancelled: Arc<Mutex<HashSet<u64>>>,
    enabled: Arc<AtomicBool>,
}

fn pipe_sddl(user_sid: &str) -> String {
    format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;{user_sid})")
}

fn user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut buf = [0u64; 32];
        let mut len = 0;
        let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), (buf.len() * 8) as u32, &mut len);
        let _ = CloseHandle(token);
        got.ok()?;
        let mut text = PWSTR::null();
        ConvertSidToStringSidW((*(buf.as_ptr() as *const TOKEN_USER)).User.Sid, &mut text).ok()?;
        text.to_string().ok()
    }
}

fn pipe_security() -> Option<SECURITY_ATTRIBUTES> {
    let sddl: Vec<u16> = pipe_sddl(&user_sid()?).encode_utf16().chain([0]).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut descriptor, None) }.ok()?;
    Some(SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor.0, bInheritHandle: false.into() })
}

impl Server {
    pub fn start(hwnd: HWND) -> Server {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let (shared, target) = (queue.clone(), hwnd.0 as isize);
        crate::system::spawn("approve-accept", move || {
            let security = pipe_security();
            loop {
                let pipe = unsafe {
                    CreateNamedPipeW(w!("\\\\.\\pipe\\puw-notch-approve"), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                        PIPE_UNLIMITED_INSTANCES, 4096, 4096, 0, security.as_ref().map(|s| s as *const SECURITY_ATTRIBUTES))
                };
                if pipe == INVALID_HANDLE_VALUE {
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
                let connected = unsafe { ConnectNamedPipe(pipe, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED };
                if !connected {
                    unsafe { let _ = CloseHandle(pipe); }
                    continue;
                }
                let (shared, pipe) = (shared.clone(), pipe.0 as isize);
                crate::system::spawn("approve-serve", move || serve(HANDLE(pipe as _), &shared, target));
            }
        });
        Server { queue, cancelled: Arc::default(), enabled: Arc::new(AtomicBool::new(true)) }
    }

    pub fn cancelled(&self) -> Arc<Mutex<HashSet<u64>>> {
        self.cancelled.clone()
    }

    pub fn enabled(&self) -> Arc<AtomicBool> {
        self.enabled.clone()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn take_cancelled(&self, id: u64) -> bool {
        self.cancelled.lock().is_ok_and(|mut c| c.remove(&id))
    }

    pub fn queue(&self) -> Arc<Mutex<VecDeque<Pending>>> {
        self.queue.clone()
    }

    pub fn next(&self) -> Option<Pending> {
        self.queue.lock().ok()?.pop_front()
    }
}

pub fn enqueue(queue: &Mutex<VecDeque<Pending>>, target: isize, app: String, title: String, detail: String, options: Vec<String>) -> (u64, Receiver<Answer>) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (reply, rx) = channel();
    if let Ok(mut q) = queue.lock() {
        q.push_back(Pending { id, app, title, detail, options, reply });
    }
    unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_APPROVE, WPARAM(0), LPARAM(0)); }
    (id, rx)
}

pub fn cancel(cancelled: &Mutex<HashSet<u64>>, target: isize, id: u64) {
    if let Ok(mut c) = cancelled.lock() {
        c.insert(id);
    }
    unsafe { let _ = PostMessageW(Some(HWND(target as _)), WM_APPROVE, WPARAM(0), LPARAM(0)); }
}

fn serve(pipe: HANDLE, queue: &Mutex<VecDeque<Pending>>, target: isize) {
    unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
    let answer = read_request(pipe).map_or(Answer::Ask, |(app, title, detail, options)| {
        enqueue(queue, target, app, title, detail, options).1.recv_timeout(TIMEOUT + Duration::from_secs(2)).unwrap_or(Answer::Ask)
    });
    unsafe {
        let mut written = 0u32;
        let _ = WriteFile(pipe, Some(&answer.line()), Some(&mut written), None);
        let _ = FlushFileBuffers(pipe);
        let _ = DisconnectNamedPipe(pipe);
        let _ = CloseHandle(pipe);
    }
}

fn read_request(pipe: HANDLE) -> Option<(String, String, String, Vec<String>)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    while !bytes.contains(&b'\n') && bytes.len() < MAX_REQUEST {
        let mut read = 0u32;
        unsafe { ReadFile(pipe, Some(&mut chunk), Some(&mut read), None).ok()? };
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read as usize]);
    }
    parse_request(&String::from_utf8_lossy(&bytes))
}

pub fn parse_request(text: &str) -> Option<(String, String, String, Vec<String>)> {
    let json = JsonObject::Parse(&text.trim().into()).ok()?;
    let field = |k: &str| json.GetNamedString(&k.into()).map(|s| s.to_string()).unwrap_or_default();
    let (app, title, detail) = (field("app"), field("title"), field("detail"));
    let options = json.GetNamedArray(&"options".into()).map(|a| (0..a.Size().unwrap_or(0)).filter_map(|i| a.GetStringAt(i).ok()).map(|s| s.to_string()).collect()).unwrap_or_default();
    (!app.is_empty()).then_some((app, title, detail, options))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipe_is_open_to_the_user_who_runs_the_notch_elevated_or_not() {
        assert_eq!(pipe_sddl("S-1-5-21-1-2-3-1001"), "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;S-1-5-21-1-2-3-1001)");
        assert!(user_sid().is_some_and(|s| s.starts_with("S-1-5-")), "the SID of this user");
        let security = pipe_security().expect("the descriptor is made");
        assert!(!security.lpSecurityDescriptor.is_null());
    }

    #[test]
    fn a_request_is_one_line_of_json() {
        let r = parse_request("{\"app\":\"Claude\",\"title\":\"Bash\",\"detail\":\"rtk git status\"}\n");
        assert_eq!(r, Some(("Claude".into(), "Bash".into(), "rtk git status".into(), Vec::new())));
        let q = parse_request("{\"app\":\"Claude\",\"title\":\"Q\",\"detail\":\"?\",\"options\":[\"Yes\",\"No\"]}");
        assert_eq!(q.map(|r| r.3), Some(vec!["Yes".to_string(), "No".to_string()]));
        assert_eq!(Answer::Choice(2).line(), b"choice:2\n".to_vec());
        assert_eq!(parse_request("not json"), None);
        assert_eq!(parse_request("{\"title\":\"x\"}"), None, "no app, no banner");
    }
}
