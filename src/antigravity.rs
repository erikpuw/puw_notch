//! Antigravity's questions to its user on the notch. While the app runs, its language server on
//! this PC (see `usage::antigravity_server`) is asked every POLL which agent runs are waiting;
//! the last step of a waiting run is either a short question with options or a permission to use
//! a tool. That becomes a banner (see approve.rs) and the click is sent back to the same server.
//! Nothing is answered without a click, and a request that is not clicked stays in Antigravity.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::HSTRING;
use windows::Data::Json::{JsonArray, JsonObject, JsonValue};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use crate::approve::{self, Answer, Pending};
use crate::i18n::tr;
use crate::usage;

const POLL: Duration = Duration::from_secs(2);
/// While Antigravity is not running, looking for it is a wasted process listing: less often.
const POLL_CLOSED: Duration = Duration::from_secs(10);

/// What a waiting step asks.
#[derive(Debug, PartialEq)]
enum Ask {
    /// A question with options: (id, text) each.
    Question { question: String, options: Vec<(String, String)> },
    /// Permission to use a tool: its kind ("mcp") and target.
    Permission { action: String, target: String },
}

/// A step of an agent run that waits for the user.
#[derive(Debug, PartialEq)]
struct Waiting {
    cascade: String,
    trajectory: String,
    step: i64,
    ask: Ask,
}

fn key(k: &str) -> HSTRING {
    HSTRING::from(k)
}

fn string(o: &JsonObject, k: &str) -> Option<String> {
    o.GetNamedString(&key(k)).ok().map(|s| s.to_string())
}

fn array_items(a: &JsonArray) -> Vec<JsonObject> {
    (0..a.Size().unwrap_or(0)).filter_map(|i| a.GetObjectAt(i).ok()).collect()
}

/// The runs that are not idle, as (cascade id, number of steps), from `GetAllCascadeTrajectories`.
fn busy_runs(text: &str) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let Some(all) = JsonObject::Parse(&key(text)).ok().and_then(|r| r.GetNamedObject(&key("trajectorySummaries")).ok()) else { return out };
    let Ok(it) = all.First() else { return out };
    while it.HasCurrent().unwrap_or(false) {
        if let (Ok(pair), true) = (it.Current(), true) {
            if let (Ok(id), Some(run)) = (pair.Key(), pair.Value().ok().and_then(|v| v.GetObject().ok())) {
                let idle = string(&run, "status").is_some_and(|s| s.contains("IDLE"));
                let steps = run.GetNamedNumber(&key("stepCount")).unwrap_or(0.0) as i64;
                if !idle && steps > 0 {
                    out.push((id.to_string(), steps));
                }
            }
        }
        if !it.MoveNext().unwrap_or(false) {
            break;
        }
    }
    out
}

/// The last step of `GetCascadeTrajectorySteps` if it waits for the user with a question (one
/// question, 1 to 3 options, a single answer) or a permission.
fn parse_waiting(text: &str) -> Option<Waiting> {
    let steps = JsonObject::Parse(&key(text)).ok()?.GetNamedArray(&key("steps")).ok()?;
    let step = steps.GetObjectAt(steps.Size().ok()?.checked_sub(1)?).ok()?;
    if !string(&step, "status")?.contains("WAITING") {
        return None;
    }
    let from = step.GetNamedObject(&key("metadata")).ok()?.GetNamedObject(&key("sourceTrajectoryStepInfo")).ok()?;
    let (cascade, trajectory, index) = (string(&from, "cascadeId")?, string(&from, "trajectoryId")?, from.GetNamedNumber(&key("stepIndex")).ok()? as i64);
    let wanted = step.GetNamedObject(&key("requestedInteraction")).ok()?;
    let ask = if let Ok(q) = wanted.GetNamedObject(&key("askQuestion")) {
        let questions = array_items(&q.GetNamedArray(&key("questions")).ok()?);
        let [only] = questions.as_slice() else { return None };
        if only.GetNamedBoolean(&key("isMultiSelect")).unwrap_or(false) {
            return None;
        }
        let options: Vec<(String, String)> = array_items(&only.GetNamedArray(&key("options")).ok()?).iter().filter_map(|o| Some((string(o, "id")?, string(o, "text")?))).collect();
        if options.is_empty() || options.len() > 3 {
            return None;
        }
        Ask::Question { question: string(only, "question")?, options }
    } else {
        let resource = wanted.GetNamedObject(&key("permission")).ok()?.GetNamedObject(&key("resource")).ok()?;
        Ask::Permission { action: string(&resource, "action")?, target: string(&resource, "target")? }
    };
    Some(Waiting { cascade, trajectory, step: index, ask })
}

/// A button label from an option's text: "(Recommended) Đồng ý: Cho phép..." -> "Đồng ý".
fn short_label(text: &str) -> String {
    let mut t = text.trim();
    if t.starts_with('(') {
        if let Some((_, rest)) = t.split_once(')') {
            t = rest.trim();
        }
    }
    let t = t.split(':').next().unwrap_or(t).trim();
    t.chars().take(14).collect()
}

/// `value` as a JSON string literal.
fn json_string(value: &str) -> String {
    JsonValue::CreateStringValue(&key(value)).and_then(|v| v.Stringify()).map_or_else(|_| "\"\"".into(), |s| s.to_string())
}

/// The body of `HandleCascadeUserInteraction` for an answer, None when it is not one to send.
fn answer_body(w: &Waiting, answer: Answer) -> Option<String> {
    let inner = match (&w.ask, answer) {
        (Ask::Permission { .. }, Answer::Allow) => r#""permission":{"allow":true,"scope":"PERMISSION_SCOPE_ONCE"}"#.to_string(),
        (Ask::Permission { .. }, Answer::Deny) => r#""permission":{"allow":false}"#.to_string(),
        (Ask::Question { question, options }, Answer::Choice(i)) => {
            let (id, _) = options.get(i)?;
            format!(r#""askQuestion":{{"responses":[{{"question":{},"selectedOptionIds":[{}]}}]}}"#, json_string(question), json_string(id))
        }
        _ => return None,
    };
    Some(format!(r#"{{"cascadeId":{},"interaction":{{"trajectoryId":{},"stepIndex":{},{inner}}}}}"#, json_string(&w.cascade), json_string(&w.trajectory), w.step))
}

/// Ask the server for the waiting steps of every run that is not idle.
fn waiting_now(server: &usage::LocalServer) -> Vec<Waiting> {
    let Some(list) = server.call("GetAllCascadeTrajectories", "{}") else { return Vec::new() };
    busy_runs(&list)
        .into_iter()
        .filter(|(id, _)| id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .filter_map(|(id, steps)| parse_waiting(&server.call("GetCascadeTrajectorySteps", &format!(r#"{{"cascadeId":"{id}","stepOffset":{}}}"#, steps - 1))?))
        .collect()
}

/// Start watching: a request for every new waiting step goes to the approval queue.
pub fn watch(queue: Arc<Mutex<VecDeque<Pending>>>, hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
        let mut seen: HashSet<(String, i64)> = HashSet::new();
        loop {
            let Some(server) = usage::antigravity_server() else {
                seen.clear();
                std::thread::sleep(POLL_CLOSED);
                continue;
            };
            std::thread::sleep(POLL);
            for waiting in waiting_now(&server) {
                if !seen.insert((waiting.cascade.clone(), waiting.step)) {
                    continue;
                }
                let (title, detail, mut options) = match &waiting.ask {
                    Ask::Question { question, options } => (tr("Câu hỏi", "Question").to_string(), question.clone(), options.iter().map(|(_, text)| short_label(text)).collect::<Vec<_>>()),
                    Ask::Permission { action, target } => (format!("{} {}", tr("Cho phép", "Allow"), action), target.clone(), Vec::new()),
                };
                if !options.is_empty() {
                    options.push(tr("Trong app", "In the app").to_string());
                }
                let rx = approve::enqueue(&queue, target, "Antigravity".into(), title, detail, options);
                // One thread per request: it waits for the click, then answers.
                std::thread::spawn(move || {
                    unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
                    let answer = rx.recv_timeout(approve::TIMEOUT + Duration::from_secs(5)).unwrap_or(Answer::Ask);
                    if let (Some(body), Some(server)) = (answer_body(&waiting, answer), usage::antigravity_server()) {
                        let _ = server.call("HandleCascadeUserInteraction", &body);
                    }
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUESTION: &str = r#"{"steps":[{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","status":"CORTEX_STEP_STATUS_DONE"},{"type":"CORTEX_STEP_TYPE_ASK_QUESTION","status":"CORTEX_STEP_STATUS_WAITING","metadata":{"sourceTrajectoryStepInfo":{"trajectoryId":"t-1","stepIndex":2,"cascadeId":"c-1"}},"requestedInteraction":{"askQuestion":{"questions":[{"question":"Cho phép?","options":[{"id":"1","text":"(Recommended) Đồng ý: Cho phép gọi công cụ"},{"id":"2","text":"Từ chối: Không chạy"}]}]}}}]}"#;
    const PERMISSION: &str = r#"{"steps":[{"type":"CORTEX_STEP_TYPE_GENERIC","status":"CORTEX_STEP_STATUS_WAITING","metadata":{"sourceTrajectoryStepInfo":{"trajectoryId":"t-1","stepIndex":7,"cascadeId":"c-1"}},"requestedInteraction":{"permission":{"resource":{"action":"mcp","target":"codebase-memory-mcp/list_projects"}}}}]}"#;

    #[test]
    fn a_waiting_question_and_permission_are_read() {
        let q = parse_waiting(QUESTION).unwrap();
        assert_eq!((q.cascade.as_str(), q.trajectory.as_str(), q.step), ("c-1", "t-1", 2));
        assert!(matches!(&q.ask, Ask::Question { question, options } if question == "Cho phép?" && options.len() == 2 && options[1].0 == "2"));
        let p = parse_waiting(PERMISSION).unwrap();
        assert_eq!(p.ask, Ask::Permission { action: "mcp".into(), target: "codebase-memory-mcp/list_projects".into() });
        assert!(parse_waiting(&QUESTION.replace("WAITING", "DONE")).is_none(), "a step that is done waits for nobody");
    }

    #[test]
    fn answers_name_the_run_the_step_and_the_choice() {
        let q = parse_waiting(QUESTION).unwrap();
        let body = answer_body(&q, Answer::Choice(1)).unwrap();
        assert_eq!(body, r#"{"cascadeId":"c-1","interaction":{"trajectoryId":"t-1","stepIndex":2,"askQuestion":{"responses":[{"question":"Cho phép?","selectedOptionIds":["2"]}]}}}"#);
        assert!(answer_body(&q, Answer::Choice(2)).is_none(), "the app's own button sends nothing");
        assert!(answer_body(&q, Answer::Ask).is_none());
        let p = parse_waiting(PERMISSION).unwrap();
        assert!(answer_body(&p, Answer::Allow).unwrap().contains(r#""permission":{"allow":true,"scope":"PERMISSION_SCOPE_ONCE"}"#));
        assert!(answer_body(&p, Answer::Deny).unwrap().contains(r#""allow":false"#));
    }

    #[test]
    fn buttons_get_short_labels() {
        assert_eq!(short_label("(Recommended) Đồng ý: Cho phép gọi công cụ get_architecture"), "Đồng ý");
        assert_eq!(short_label("Từ chối: Không chạy công cụ này"), "Từ chối");
        assert_eq!(short_label("Một lựa chọn rất là dài không có dấu hai chấm"), "Một lựa chọn r");
    }

    #[test]
    fn busy_runs_skip_the_idle_ones() {
        let text = r#"{"trajectorySummaries":{"a-1":{"status":"CASCADE_RUN_STATUS_IDLE","stepCount":610},"b-2":{"status":"CASCADE_RUN_STATUS_RUNNING","stepCount":3}}}"#;
        assert_eq!(busy_runs(text), vec![("b-2".to_string(), 3)]);
    }
}
