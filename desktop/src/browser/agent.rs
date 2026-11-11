//! What an agent's browser tools do to a page, through the DevTools protocol.
//!
//! The host relays each tool call here as a command (see `browser_relay` in
//! xd-host). [`run`] carries it out on one page and words the result for the
//! agent. It blocks, so it runs on a thread of its own.

use std::{
    cell::Cell,
    sync::{
        Mutex, MutexGuard, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// The DevTools calls a command needs. Chromium's own is
/// [`super::chromium::DevTools`]; tests use a script.
pub trait Page {
    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String>;

    /// Whether the page is loading, including a document it has asked for
    /// and not yet received.
    fn is_loading(&self) -> bool {
        false
    }

    /// How long the command may still take.
    fn time_left(&self) -> Duration {
        Duration::MAX
    }

    /// Whether the last call reached the page, or may have: false for one
    /// refused or stopped before it was sent.
    fn last_call_sent(&self) -> bool {
        true
    }

    /// Calls `method` even once the command is stopped or out of time: for
    /// letting go of a button or key it pressed, which must not stay held.
    fn call_regardless(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.call(method, params, timeout)
    }
}

#[cfg(target_os = "linux")]
impl Page for super::chromium::DevTools {
    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        super::chromium::DevTools::call(self, method, params, timeout)
    }

    fn is_loading(&self) -> bool {
        super::chromium::DevTools::is_loading(self)
    }

    fn last_call_sent(&self) -> bool {
        super::chromium::DevTools::last_call_sent(self)
    }

    fn call_regardless(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.unstoppable().call(method, params, timeout)
    }
}

/// A page whose calls stop at a command's deadline, or once the host gives
/// up on the command, so it cannot go on to click or submit anything.
struct Bounded<'a, P> {
    page: &'a P,
    deadline: Instant,
    cancelled: &'a AtomicBool,
    /// The last call was refused here, so never sent.
    refused: Cell<bool>,
}

impl<P: Page> Page for Bounded<'_, P> {
    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.refused.set(true);
        if self.cancelled.load(Ordering::Acquire) {
            return Err(CANCELLED.into());
        }
        let left = self.time_left();
        if left.is_zero() {
            return Err(OUT_OF_TIME.into());
        }
        self.refused.set(false);
        self.page.call(method, params, timeout.min(left))
    }

    fn last_call_sent(&self) -> bool {
        !self.refused.get() && self.page.last_call_sent()
    }

    fn is_loading(&self) -> bool {
        self.page.is_loading()
    }

    fn call_regardless(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.page.call_regardless(method, params, timeout)
    }

    /// None once the command is stopped, which ends its waits too.
    fn time_left(&self) -> Duration {
        if self.cancelled.load(Ordering::Acquire) {
            return Duration::ZERO;
        }
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// How long a whole command may take, from when the desktop gets it. It ends
/// before the host stops waiting, so the agent hears how it went.
pub const COMMAND_TIME: Duration = Duration::from_secs(90);
const OUT_OF_TIME: &str = "The browser command ran out of time, so xd stopped it.";
const CANCELLED: &str = "The host stopped waiting for this browser command, so xd stopped it.";
/// How long one DevTools call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long letting go of a button or key may take.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a command waits for a page it opened to finish loading.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a click or key waits for a navigation it started.
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a click or key waits to see whether it starts a navigation.
const SETTLE_TIME: Duration = Duration::from_millis(600);

/// Waits for the page's other commands to finish, so two agents sharing it
/// cannot type into each other's fields. Gives up at `deadline`, or once
/// `cancelled`.
pub fn take_turn<'a>(
    turns: &'a Mutex<()>,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<MutexGuard<'a, ()>, String> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(CANCELLED.into());
        }
        match turns.try_lock() {
            Ok(turn) => return Ok(turn),
            Err(TryLockError::Poisoned(turn)) => return Ok(turn.into_inner()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(TryLockError::WouldBlock) => {
                return Err("Another browser command kept this page busy too long.".into());
            }
        }
    }
}
/// The most text a snapshot or a value returns.
const MAX_TEXT: usize = 40_000;
/// The tallest full-page screenshot, in CSS pixels.
const MAX_SCREENSHOT_HEIGHT: f64 = 6_000.0;

/// What a finished command returns: text, and for a screenshot, a picture.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub text: String,
    pub image: Option<(String, &'static str)>,
}

impl Outcome {
    fn text(text: String) -> Self {
        Self { text, image: None }
    }

    pub fn into_value(self) -> Value {
        let mut value = json!({"text": self.text});
        if let Some((data, mime_type)) = self.image {
            value["image"] = json!({"data": data, "mime_type": mime_type});
        }
        value
    }
}

/// Carries out `command` on `page`, by `deadline` and until `cancelled`. The
/// address of a `navigate` command must already be checked; see
/// `normalize_url`.
pub fn run(
    page: &impl Page,
    command: &Value,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<Outcome, String> {
    let page = &Bounded {
        page,
        deadline,
        cancelled,
        refused: Cell::new(false),
    };
    let text = |key: &str| command.get(key).and_then(Value::as_str);
    let flag = |key: &str| command.get(key).and_then(Value::as_bool) == Some(true);
    match text("action").unwrap_or_default() {
        "navigate" => {
            let url = text("url").ok_or("Pass url, the address to open.")?;
            let loading = navigate(page, url)?;
            settled_snapshot(page, loading)
        }
        "history" => {
            let loading = history(page, text("direction").unwrap_or_default())?;
            settled_snapshot(page, loading)
        }
        "snapshot" => snapshot(page),
        "click" => {
            let target = element(command)?;
            let before = mark(page)?;
            click(page, &target, flag("double"))?;
            let loading = settle(page, &before, SETTLE_TIME);
            settled_snapshot(page, loading)
        }
        "type" => {
            let target = element(command)?;
            let typed = text("text").ok_or("Pass text, what to type.")?;
            // Marked first: typing alone can send the page on, as a code
            // field that submits itself does.
            let before = mark(page)?;
            type_text(page, &target, typed, flag("append"))?;
            if flag("submit") {
                press(page, "Enter")?;
            }
            let loading = settle(page, &before, SETTLE_TIME);
            settled_snapshot(page, loading)
        }
        "press_key" => {
            let key = text("key").ok_or("Pass key, the key to press.")?;
            let before = mark(page)?;
            press(page, key)?;
            let loading = settle(page, &before, SETTLE_TIME);
            settled_snapshot(page, loading)
        }
        "evaluate" => {
            let expression = text("expression").ok_or("Pass expression, the JavaScript to run.")?;
            evaluate_for_agent(page, expression).map(Outcome::text)
        }
        "screenshot" => screenshot(page, flag("full_page")),
        _ => Err("xd's browser has no such command.".into()),
    }
}

/// The element ref a command names, such as e12, as its number.
fn element(command: &Value) -> Result<String, String> {
    let reference = command
        .get("ref")
        .and_then(Value::as_str)
        .ok_or("Pass ref, an element's ref from browser_snapshot.")?;
    let number = reference
        .trim()
        .trim_start_matches(['[', 'e'])
        .trim_end_matches(']');
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "{reference} is not an element ref. Refs look like e12; take a browser_snapshot for them."
        ));
    }
    Ok(number.to_owned())
}

/// Runs `expression` in the page and returns its value, or why it failed.
fn evaluate(page: &impl Page, expression: &str) -> Result<Value, String> {
    let reply = page.call(
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true, "awaitPromise": true,
            "userGesture": true}),
        CALL_TIMEOUT,
    )?;
    if let Some(details) = reply.get("exceptionDetails") {
        let message = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("The script failed.");
        return Err(message.to_owned());
    }
    Ok(reply["result"]["value"].clone())
}

fn evaluate_for_agent(page: &impl Page, expression: &str) -> Result<String, String> {
    let reply = page.call(
        "Runtime.evaluate",
        // Not in REPL mode, which hands back a promise the expression gives
        // rather than what it settles to.
        json!({"expression": expression, "returnByValue": true, "awaitPromise": true,
            "userGesture": true}),
        CALL_TIMEOUT,
    )?;
    if let Some(details) = reply.get("exceptionDetails") {
        let message = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("The script failed.");
        return Err(format!("The script threw: {message}"));
    }
    let result = &reply["result"];
    let text = match result.get("value") {
        Some(Value::String(text)) => text.clone(),
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_default(),
        None => result["description"]
            .as_str()
            .or_else(|| result["type"].as_str())
            .unwrap_or("undefined")
            .to_owned(),
    };
    Ok(clip(text))
}

fn clip(mut text: String) -> String {
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… (cut short)");
    }
    text
}

/// Returns whether the page was still loading when the wait for it ended.
fn navigate(page: &impl Page, url: &str) -> Result<bool, String> {
    // Answered once the new page is committed, or with why it could not be.
    let reply = page.call("Page.navigate", json!({"url": url}), LOAD_TIMEOUT)?;
    if let Some(error) = reply["errorText"]
        .as_str()
        .filter(|error| !error.is_empty())
    {
        return Err(format!("Could not open {url}: {error}"));
    }
    wait_for_load(page, LOAD_TIMEOUT)
}

/// Returns whether the page was still loading when the wait for it ended.
fn history(page: &impl Page, direction: &str) -> Result<bool, String> {
    let before = mark(page)?;
    match direction {
        "reload" => {
            page.call("Page.reload", json!({}), CALL_TIMEOUT)?;
        }
        "back" | "forward" => {
            let history = page.call("Page.getNavigationHistory", json!({}), CALL_TIMEOUT)?;
            let current = history["currentIndex"].as_i64().unwrap_or_default();
            let index = if direction == "back" {
                current - 1
            } else {
                current + 1
            };
            let entry = usize::try_from(index)
                .ok()
                .and_then(|index| history["entries"].get(index))
                .ok_or_else(|| format!("There is no page to go {direction} to."))?;
            page.call(
                "Page.navigateToHistoryEntry",
                json!({"entryId": entry["id"]}),
                CALL_TIMEOUT,
            )?;
        }
        _ => return Err("Pass direction: back, forward or reload.".into()),
    }
    Ok(settle(page, &before, SETTLE_TIME))
}

/// Marks the current document, so [`settle`] can tell when another
/// replaces it.
fn mark(page: &impl Page) -> Result<String, String> {
    static MARKS: AtomicU64 = AtomicU64::new(0);
    let mark = format!(
        "xd-{}-{}",
        std::process::id(),
        MARKS.fetch_add(1, Ordering::Relaxed)
    );
    evaluate(page, &format!("window.__xdBrowserMark = {mark:?}"))?;
    Ok(mark)
}

/// Waits for whatever an action started: a navigation to finish, or, when
/// none has begun within `quiet`, nothing. A navigation still waiting for its
/// server keeps the old document, and a page that sends the browser straight
/// on to another shows complete while that one loads, so the page's loading
/// state is what tells either is under way; a page that keeps loading is
/// waited for until the time runs out. Returns whether the page was still
/// loading when the wait ended.
fn settle(page: &impl Page, mark: &str, quiet: Duration) -> bool {
    let started = Instant::now();
    let end = started + NAVIGATION_TIMEOUT.min(page.time_left());
    loop {
        // Fails while one document gives way to the next.
        let state = evaluate(
            page,
            &format!("[window.__xdBrowserMark === {mark:?}, document.readyState]"),
        );
        // Looked at after the document, so a navigation that began while
        // reading it counts.
        let loading = page.is_loading();
        match state.as_ref().ok().and_then(Value::as_array) {
            Some(state) if !loading && state[0] == true && started.elapsed() >= quiet => {
                return false;
            }
            Some(state) if !loading && state[0] == false && state[1] == "complete" => {
                // A page may send the browser on as soon as it has loaded.
                thread::sleep(Duration::from_millis(200));
                if !page.is_loading() {
                    return false;
                }
            }
            _ => {}
        }
        if Instant::now() >= end || page.time_left().is_zero() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// A snapshot, noting a page that had not finished loading.
fn settled_snapshot(page: &impl Page, loading: bool) -> Result<Outcome, String> {
    let mut outcome = snapshot(page)?;
    if loading {
        outcome.text = format!(
            "The page was still loading when xd stopped waiting, so it may change.\n\n{}",
            outcome.text
        );
    }
    Ok(outcome)
}

/// Returns whether the page was still loading when the wait ended, as when it
/// sent the browser on to another page or a slow resource holds up its load
/// event. The page may be usable then, so that is no failure.
fn wait_for_load(page: &impl Page, timeout: Duration) -> Result<bool, String> {
    let started = Instant::now();
    let timeout = timeout.min(page.time_left());
    while started.elapsed() < timeout && !page.time_left().is_zero() {
        if !page.is_loading()
            && evaluate(page, "document.readyState").ok() == Some(json!("complete"))
        {
            // A page may send the browser on as soon as it has loaded.
            thread::sleep(Duration::from_millis(200));
            if !page.is_loading() {
                return Ok(false);
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(true)
}

/// The most refs one snapshot hands out. Its text limit allows far fewer.
const REFS_PER_SNAPSHOT: u64 = 5_000;

fn snapshot(page: &impl Page) -> Result<Outcome, String> {
    // Numbers no snapshot has used, on any page, so a ref another agent was
    // given can only ever name the element it was given for.
    static NEXT_REF: AtomicU64 = AtomicU64::new(0);
    let first = NEXT_REF.fetch_add(REFS_PER_SNAPSHOT, Ordering::Relaxed);
    let value = evaluate(
        page,
        &format!("({SNAPSHOT_SCRIPT})({MAX_TEXT}, {first}, {REFS_PER_SNAPSHOT})"),
    )?;
    let field = |key: &str| value[key].as_str().unwrap_or_default();
    Ok(Outcome::text(format!(
        "Page: {}\nAddress: {}\n\n{}",
        field("title"),
        field("url"),
        field("text")
    )))
}

/// Scrolls element `number` into view and returns its centre, in CSS pixels
/// from the top left of the view, or why it cannot be clicked there.
fn locate(page: &impl Page, number: &str) -> Result<(f64, f64), String> {
    let found = evaluate(
        page,
        &format!(
            "(() => {{
                const refs = window.__xdBrowserRefs;
                const element = refs instanceof Map ? refs.get({number}) : undefined;
                if (!element || !element.isConnected) return {{missing: true}};
                element.scrollIntoView({{block: 'center', inline: 'center', behavior: 'instant'}});
                const box = element.getBoundingClientRect();
                const x = box.left + box.width / 2, y = box.top + box.height / 2;
                if (box.width === 0 || box.height === 0) return {{hidden: true}};
                // Through shadow roots, which hide their insides from the page,
                // and from slotted content to the slot that shows it.
                const within = (outer, node) => {{
                    for (let at = node; at; at = at.assignedSlot || at.parentNode || at.host) if (at === outer) return true;
                    return false;
                }};
                let top = document.elementFromPoint(x, y);
                while (top && top.shadowRoot) {{
                    const inner = top.shadowRoot.elementFromPoint(x, y);
                    if (!inner || inner === top) break;
                    top = inner;
                }}
                // Only the element, what it holds, or its label take its
                // click; one that lets clicks through to its container does
                // not get it.
                // Through a label, unless the point is on another control in
                // it, such as a link, which takes the click for itself. Looked
                // for through shadow roots, as `within` looks.
                const controlAt = node => {{
                    for (let at = node; at; at = at.assignedSlot || at.parentNode || at.host)
                        if (at.nodeType === 1 && (at.isContentEditable || at.matches(
                            'a[href], button, input, select, textarea, summary, '
                            + ['button', 'link', 'checkbox', 'radio', 'switch', 'tab', 'menuitem',
                               'menuitemcheckbox', 'menuitemradio', 'option', 'combobox', 'textbox',
                               'searchbox', 'slider', 'spinbutton', 'treeitem']
                                .map(role => `[role=${{role}}]`).join(', '))))
                            return at;
                    return null;
                }};
                const control = top && controlAt(top);
                // Inside the element, or its label, unless the point is on
                // another control in there, such as a nested tree item or a
                // label's link, which takes the click for itself.
                const own = holder => within(holder, top)
                    && (!control || control === element || !within(holder, control));
                const reaches = top && (own(element)
                    || (element.labels && [...element.labels].some(own)));
                const describe = node => node.tagName.toLowerCase()
                    + (node.id ? '#' + node.id : '')
                    + (typeof node.className === 'string' && node.className.trim()
                        ? '.' + node.className.trim().split(/\\s+/).slice(0, 2).join('.') : '');
                return {{x, y, covered: reaches ? null : top ? describe(top) : 'nothing'}};
            }})()"
        ),
    )?;
    if found["missing"] == true {
        return Err(format!(
            "e{number} is not on the page any more. Take a new browser_snapshot for current refs."
        ));
    }
    if found["hidden"] == true {
        return Err(format!(
            "e{number} takes up no space on the page, so it cannot be clicked."
        ));
    }
    if let Some(cover) = found["covered"].as_str() {
        return Err(format!(
            "A click on the middle of e{number} would reach {cover}, not e{number}. Close or \
             scroll past what covers it first, or click it with browser_evaluate."
        ));
    }
    match (found["x"].as_f64(), found["y"].as_f64()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("Could not find where e{number} is.")),
    }
}

fn click(page: &impl Page, number: &str, double: bool) -> Result<(), String> {
    let (x, y) = locate(page, number)?;
    let mouse = |kind: &str, count: u32| {
        json!({"type": kind, "x": x, "y": y,
            "button": if kind == "mouseMoved" { "none" } else { "left" },
            "buttons": if kind == "mousePressed" { 1 } else { 0 }, "clickCount": count})
    };
    page.call(
        "Input.dispatchMouseEvent",
        mouse("mouseMoved", 0),
        CALL_TIMEOUT,
    )?;
    for count in 1..=if double { 2 } else { 1 } {
        held(
            page,
            || {
                page.call(
                    "Input.dispatchMouseEvent",
                    mouse("mousePressed", count),
                    CALL_TIMEOUT,
                )
            },
            || {
                page.call_regardless(
                    "Input.dispatchMouseEvent",
                    mouse("mouseReleased", count),
                    RELEASE_TIMEOUT,
                )
            },
        )?;
    }
    Ok(())
}

/// Presses with `down` and lets go with `up`. Once `down` has reached the
/// page, or may have, `up` is sent whatever became of it, even for a command
/// stopped or out of time meanwhile, so no button or key is left held; one
/// never sent gets no `up`, which the page could act on instead.
fn held(
    page: &impl Page,
    down: impl FnOnce() -> Result<Value, String>,
    up: impl FnOnce() -> Result<Value, String>,
) -> Result<(), String> {
    let pressed = down();
    if pressed.is_err() && !page.last_call_sent() {
        return pressed.map(|_| ());
    }
    let released = up();
    pressed.and(released).map(|_| ())
}

fn type_text(page: &impl Page, number: &str, text: &str, append: bool) -> Result<(), String> {
    let focused = evaluate(
        page,
        &format!(
            "(() => {{
                const refs = window.__xdBrowserRefs;
                const element = refs instanceof Map ? refs.get({number}) : undefined;
                if (!element || !element.isConnected) return 'missing';
                element.scrollIntoView({{block: 'center', behavior: 'instant'}});
                element.focus();
                // Focus inside a shadow root shows on the page as its host.
                let active = document.activeElement;
                while (active && active.shadowRoot && active.shadowRoot.activeElement)
                    active = active.shadowRoot.activeElement;
                const within = (outer, node) => {{
                    for (let at = node; at; at = at.assignedSlot || at.parentNode || at.host) if (at === outer) return true;
                    return false;
                }};
                if (!active || !within(element, active)) return 'unfocusable';
                if (element.isContentEditable || element.matches('input, textarea')) return 'ok';
                return 'not-text';
            }})()"
        ),
    )?;
    match focused.as_str() {
        Some("ok") => {}
        Some("missing") => {
            return Err(format!(
                "e{number} is not on the page any more. Take a new browser_snapshot for current refs."
            ));
        }
        Some("not-text") => {
            return Err(format!(
                "e{number} is not a text field. Click it, then use browser_press_key."
            ));
        }
        _ => return Err(format!("e{number} cannot take the keyboard focus.")),
    }
    if append {
        // To the end of what is there, which the editor does for every kind
        // of field, where the selection API has no say over some inputs.
        chord(page, &key("End")?, 2, &["moveToEndOfDocument"])?;
    } else {
        // Select what is there, so the typed text replaces it.
        chord(page, &key("a")?, 2, &["selectAll"])?;
        if text.is_empty() {
            press(page, "Delete")?;
        }
    }
    if !text.is_empty() {
        page.call("Input.insertText", json!({"text": text}), CALL_TIMEOUT)?;
    }
    Ok(())
}

/// A key as the DevTools protocol describes it.
#[derive(Debug, PartialEq)]
struct Key {
    key: String,
    code: String,
    key_code: u32,
    text: Option<String>,
}

fn key(name: &str) -> Result<Key, String> {
    let named = |key: &str, code: &str, key_code: u32, text: Option<&str>| Key {
        key: key.into(),
        code: code.into(),
        key_code,
        text: text.map(str::to_owned),
    };
    let lower = name.to_ascii_lowercase();
    Ok(match lower.as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "backspace" => named("Backspace", "Backspace", 8, None),
        "delete" => named("Delete", "Delete", 46, None),
        "space" | " " => named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" => named("PageUp", "PageUp", 33, None),
        "pagedown" => named("PageDown", "PageDown", 34, None),
        _ => {
            let mut characters = name.chars();
            let (Some(character), None) = (characters.next(), characters.next()) else {
                return Err(format!(
                    "{name} is not a key xd knows. Use a key name such as Enter, Escape, Tab, \
                     Backspace, ArrowDown or PageDown, or a single character."
                ));
            };
            let upper = character.to_ascii_uppercase();
            let (code, key_code) = if upper.is_ascii_alphabetic() {
                (format!("Key{upper}"), u32::from(upper))
            } else if character.is_ascii_digit() {
                (format!("Digit{character}"), u32::from(character))
            } else {
                (String::new(), 0)
            };
            Key {
                key: character.to_string(),
                code,
                key_code,
                text: Some(character.to_string()),
            }
        }
    })
}

/// The DevTools event for pressing or letting go of `key`.
fn key_event(kind: &str, key: &Key, modifiers: u32, commands: &[&str]) -> Value {
    let mut event = json!({"type": kind, "key": key.key, "code": key.code,
        "windowsVirtualKeyCode": key.key_code, "nativeVirtualKeyCode": key.key_code,
        "modifiers": modifiers});
    if kind == "keyDown"
        && let Some(text) = &key.text
    {
        event["text"] = json!(text);
        event["unmodifiedText"] = json!(text);
    }
    if !commands.is_empty() {
        event["commands"] = json!(commands);
    }
    event
}

/// Presses `key` with `modifiers` held, running the editor's `commands`.
fn chord(page: &impl Page, key: &Key, modifiers: u32, commands: &[&str]) -> Result<(), String> {
    held(
        page,
        || {
            page.call(
                "Input.dispatchKeyEvent",
                key_event("rawKeyDown", key, modifiers, commands),
                CALL_TIMEOUT,
            )
        },
        || {
            page.call_regardless(
                "Input.dispatchKeyEvent",
                key_event("keyUp", key, modifiers, &[]),
                RELEASE_TIMEOUT,
            )
        },
    )
}

fn press(page: &impl Page, name: &str) -> Result<(), String> {
    let key = key(name)?;
    let down = if key.text.is_some() {
        "keyDown"
    } else {
        "rawKeyDown"
    };
    held(
        page,
        || {
            page.call(
                "Input.dispatchKeyEvent",
                key_event(down, &key, 0, &[]),
                CALL_TIMEOUT,
            )
        },
        || {
            page.call_regardless(
                "Input.dispatchKeyEvent",
                key_event("keyUp", &key, 0, &[]),
                RELEASE_TIMEOUT,
            )
        },
    )
}

fn screenshot(page: &impl Page, full_page: bool) -> Result<Outcome, String> {
    let mut params = json!({"format": "jpeg", "quality": 80});
    let mut note = String::new();
    if full_page {
        let metrics = page.call("Page.getLayoutMetrics", json!({}), CALL_TIMEOUT)?;
        let size = &metrics["cssContentSize"];
        let width = size["width"].as_f64().unwrap_or(0.0).ceil();
        let mut height = size["height"].as_f64().unwrap_or(0.0).ceil();
        if height > MAX_SCREENSHOT_HEIGHT {
            note = format!(
                " The page is {height} pixels tall, so this shows its first {MAX_SCREENSHOT_HEIGHT}."
            );
            height = MAX_SCREENSHOT_HEIGHT;
        }
        if width > 0.0 && height > 0.0 {
            params["captureBeyondViewport"] = json!(true);
            params["clip"] = json!({"x": 0, "y": 0, "width": width, "height": height, "scale": 1});
        }
    }
    let reply = page.call("Page.captureScreenshot", params, CALL_TIMEOUT)?;
    let data = reply["data"]
        .as_str()
        .filter(|data| !data.is_empty())
        .ok_or("The page gave no picture.")?;
    let address = evaluate(page, "location.href")
        .ok()
        .and_then(|address| address.as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(Outcome {
        text: format!("Screenshot of {address}.{note}"),
        image: Some((data.to_owned(), "image/jpeg")),
    })
}

/// Reads the page as text, numbering its controls so commands can name them.
/// Takes the most text to return, the first number to use and how many it
/// may. Adds the controls to `window.__xdBrowserRefs`, by number, which keeps
/// those of recent snapshots, since another agent may still use them.
const SNAPSHOT_SCRIPT: &str = r#"(limit, first, most) => {
  if (!(window.__xdBrowserRefs instanceof Map)) window.__xdBrowserRefs = new Map();
  const refs = window.__xdBrowserRefs;
  let next = first;
  const lines = [];
  let size = 0;
  let full = false;
  let text = '';
  // Inside something whose text is already a line's, such as a control's
  // label, a clickable area or a heading: only the controls in it are added.
  let naming = 0;
  const squash = value => (value || '').replace(/\s+/g, ' ').trim();
  const short = (value, most) => {
    value = squash(value);
    return value.length > most ? value.slice(0, most - 1) + '…' : value;
  };
  const add = line => {
    if (full) return;
    if (size + line.length > limit) {
      full = true;
      lines.push('… (the page goes on; use browser_evaluate to read the rest)');
      return;
    }
    lines.push(line);
    size += line.length + 1;
  };
  const flush = () => {
    const line = short(text, 2000);
    text = '';
    if (line) add(line);
  };
  const skipped = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'HEAD', 'META', 'LINK']);

  const roles = new Set(['button', 'link', 'checkbox', 'radio', 'switch', 'tab', 'menuitem',
    'menuitemcheckbox', 'menuitemradio', 'option', 'combobox', 'textbox', 'searchbox', 'slider',
    'spinbutton', 'treeitem']);
  const labelled = element => {
    const ids = element.getAttribute('aria-labelledby');
    if (ids) {
      // Ids name elements in the element's own tree, which a shadow root is.
      const root = element.getRootNode();
      const byId = id => (root.getElementById ? root.getElementById(id) : null);
      const label = ids.split(/\s+/).map(byId)
        .filter(Boolean).map(node => node.innerText || node.textContent).join(' ');
      if (squash(label)) return label;
    }
    if (element.getAttribute('aria-label')) return element.getAttribute('aria-label');
    if (element.labels && element.labels.length)
      return [...element.labels].map(label => label.innerText).join(' ');
    return '';
  };
  const kind = (element, tag, style) => {
    const role = (element.getAttribute('role') || '').toLowerCase();
    if (roles.has(role)) return role;
    if (tag === 'A' && (element.hasAttribute('href') || element.hasAttribute('xlink:href')))
      return 'link';
    if (tag === 'BUTTON' || tag === 'SUMMARY') return 'button';
    if (tag === 'SELECT') return 'combobox';
    if (tag === 'TEXTAREA') return 'textbox';
    if (tag === 'INPUT') {
      const type = (element.getAttribute('type') || 'text').toLowerCase();
      if (type === 'hidden') return null;
      if (['button', 'submit', 'reset', 'image'].includes(type)) return 'button';
      if (type === 'checkbox' || type === 'radio') return type;
      if (type === 'range') return 'slider';
      if (type === 'file') return 'file chooser';
      return type === 'text' ? 'textbox' : `textbox (${type})`;
    }
    if (element.isContentEditable && !(element.parentElement && element.parentElement.isContentEditable))
      return 'textbox';
    if (style.cursor === 'pointer' && element.parentElement
      && getComputedStyle(element.parentElement).cursor !== 'pointer') return 'clickable';
    return null;
  };
  const describe = (element, tag, role) => {
    let name = labelled(element);
    if (!name && tag === 'INPUT' && ['button', 'submit', 'reset'].includes(element.type))
      name = element.value;
    if (!name && !['textbox', 'combobox', 'searchbox'].includes(role.split(' ')[0]))
      name = element.innerText;
    if (!name) name = element.getAttribute('title') || element.getAttribute('placeholder')
      || element.getAttribute('alt') || (element.querySelector('img[alt]') || {}).alt
      // An icon's own name, as an icon-only button has.
      || (element.querySelector('[aria-label]') || {getAttribute: () => ''}).getAttribute('aria-label')
      || (element.querySelector('svg title') || {}).textContent || '';
    let line = `[e${next}] ${role}`;
    name = short(name, 120);
    if (name) line += ` "${name}"`;
    if (tag === 'A') {
      const href = element.getAttribute('href') || element.getAttribute('xlink:href') || '';
      if (href && !href.startsWith('javascript:')) line += ` → ${short(href, 120)}`;
    }
    if (tag === 'SELECT') {
      const chosen = [...element.selectedOptions].map(option => option.text).join(', ');
      line += ` value="${short(chosen, 80)}"`;
      const options = [...element.options].map(option => short(option.text, 40));
      line += ` options: ${short(options.join(' | '), 300)}`;
    } else if (tag === 'INPUT' || tag === 'TEXTAREA') {
      if (element.type === 'checkbox' || element.type === 'radio') {
        if (element.checked) line += ' checked';
      } else if (element.type === 'password') {
        if (element.value) line += ' value=(hidden)';
      } else if (element.value) {
        line += ` value="${short(element.value, 200)}"`;
      }
      if (element.placeholder && name !== short(element.placeholder, 120))
        line += ` placeholder="${short(element.placeholder, 80)}"`;
    } else if (element.isContentEditable && role === 'textbox') {
      const value = short(element.innerText, 200);
      if (value) line += ` value="${value}"`;
    }
    const aria = (name, word) => {
      const value = element.getAttribute(name);
      if (value === 'true') line += ` ${word}`;
    };
    aria('aria-checked', 'checked');
    aria('aria-selected', 'selected');
    aria('aria-expanded', 'expanded');
    aria('aria-pressed', 'pressed');
    if (element.disabled || element.getAttribute('aria-disabled') === 'true') line += ' disabled';
    return line;
  };
  const children = node => {
    if (node.tagName === 'SLOT') {
      const assigned = node.assignedNodes({flatten: true});
      if (assigned.length) return assigned;
    }
    return node.shadowRoot ? node.shadowRoot.childNodes : node.childNodes;
  };
  const walk = node => {
    if (full) return;
    if (node.nodeType === Node.TEXT_NODE) {
      if (!naming) text += ' ' + node.data;
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const element = node;
    // SVG's tag names are lower case.
    const tag = element.tagName.toUpperCase();
    if (skipped.has(tag) || element.hidden || element.getAttribute('aria-hidden') === 'true') return;
    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse')
      return;
    if (tag === 'IFRAME' || tag === 'FRAME') {
      flush();
      add(`frame ${short(element.getAttribute('title') || element.src, 120)} (not read; open its address to read it)`);
      return;
    }
    // Images and drawings included, which can be buttons too.
    const role = kind(element, tag, style);
    if (role && next < first + most) {
      const box = element.getBoundingClientRect();
      if (box.width > 0 && box.height > 0) {
        flush();
        add(describe(element, tag, role));
        refs.set(next, element);
        next += 1;
        // A control's own text is its name, but it may hold more controls,
        // as an expanded tree item or a clickable card does, which are found
        // by walking it, shadow roots and slots included.
        naming += 1;
        for (const child of children(element)) walk(child);
        naming -= 1;
        return;
      }
    }
    if (tag === 'IMG') {
      const alt = short(element.getAttribute('alt'), 120);
      if (alt && !naming) text += ` [image: ${alt}]`;
      return;
    }
    if (tag === 'SVG') {
      // A drawing's text is not for reading, but it may hold links.
      naming += 1;
      for (const child of children(element)) walk(child);
      naming -= 1;
      return;
    }
    const heading = /^H([1-6])$/.exec(tag);
    if (heading) {
      flush();
      const title = short(element.innerText, 300);
      if (title && !naming) add('#'.repeat(Number(heading[1])) + ' ' + title);
      // A heading's links, as in a list of articles, are still controls.
      naming += 1;
      for (const child of children(element)) walk(child);
      naming -= 1;
      return;
    }
    const block = !style.display.startsWith('inline') && style.display !== 'contents';
    if (block || tag === 'BR') flush();
    if (tag === 'LI' && !naming) text += '- ';
    // A styled control often hides its input; then the label is what shows.
    const label = tag === 'LABEL' && element.control
      && element.control.getClientRects().length ? 1 : 0;
    naming += label;
    for (const child of children(element)) walk(child);
    naming -= label;
    if (block) flush();
  };
  walk(document.body || document.documentElement);
  flush();
  // Keep the newest; a ref older than these says to take a new snapshot.
  for (const number of refs.keys()) {
    if (refs.size <= 20000) break;
    refs.delete(number);
  }
  return {url: location.href, title: document.title, text: lines.join('\n')};
}"#;

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque};

    use super::*;

    /// Answers DevTools calls from a script, and records them.
    struct Script {
        answers: RefCell<VecDeque<(&'static str, Result<Value, String>)>>,
        calls: RefCell<Vec<(String, Value)>>,
        /// Whether the page is loading, each time it is asked; then not.
        loading: RefCell<VecDeque<bool>>,
        /// What `Runtime.evaluate` answers once the script runs out.
        fallback: Option<Value>,
    }

    impl Script {
        fn new(answers: Vec<(&'static str, Result<Value, String>)>) -> Self {
            Self {
                answers: RefCell::new(answers.into()),
                calls: RefCell::default(),
                loading: RefCell::default(),
                fallback: None,
            }
        }

        fn methods(&self) -> Vec<String> {
            self.calls
                .borrow()
                .iter()
                .map(|(method, _)| method.clone())
                .collect()
        }
    }

    impl Page for Script {
        fn call(&self, method: &str, params: Value, _: Duration) -> Result<Value, String> {
            self.calls.borrow_mut().push((method.to_owned(), params));
            let next = self.answers.borrow_mut().pop_front();
            let (expected, answer) = match (next, &self.fallback) {
                (Some(next), _) => next,
                (None, Some(fallback)) => ("Runtime.evaluate", value(fallback.clone())),
                (None, None) => panic!("no answer left for {method}"),
            };
            assert_eq!(method, expected);
            answer
        }

        fn is_loading(&self) -> bool {
            self.loading.borrow_mut().pop_front().unwrap_or(false)
        }
    }

    fn run(page: &Script, command: &Value) -> Result<Outcome, String> {
        super::run(
            page,
            command,
            Instant::now() + COMMAND_TIME,
            &AtomicBool::new(false),
        )
    }

    fn value(value: Value) -> Result<Value, String> {
        Ok(json!({"result": {"type": "object", "value": value}}))
    }

    fn page() -> Result<Value, String> {
        value(json!({"url": "http://localhost:3000/", "title": "Shop",
            "text": "# Shop\n[e0] button \"Buy\""}))
    }

    #[test]
    fn navigating_waits_for_the_page_then_reads_it() {
        let script = Script::new(vec![
            (
                "Page.navigate",
                Ok(json!({"frameId": "f", "loaderId": "l"})),
            ),
            ("Runtime.evaluate", value(json!("loading"))),
            ("Runtime.evaluate", value(json!("complete"))),
            ("Runtime.evaluate", page()),
        ]);
        let outcome = run(
            &script,
            &json!({"action": "navigate", "url": "http://localhost:3000/"}),
        )
        .unwrap();
        assert_eq!(
            outcome.text,
            "Page: Shop\nAddress: http://localhost:3000/\n\n# Shop\n[e0] button \"Buy\""
        );
        assert_eq!(
            script.calls.borrow()[0].1,
            json!({"url": "http://localhost:3000/"})
        );
    }

    #[test]
    fn a_page_that_cannot_open_says_why() {
        let script = Script::new(vec![(
            "Page.navigate",
            Ok(json!({"frameId": "f", "errorText": "net::ERR_CONNECTION_REFUSED"})),
        )]);
        assert_eq!(
            run(
                &script,
                &json!({"action": "navigate", "url": "http://localhost:1/"})
            ),
            Err("Could not open http://localhost:1/: net::ERR_CONNECTION_REFUSED".into())
        );
    }

    #[test]
    fn clicking_presses_the_mouse_on_the_middle_of_the_element() {
        let script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            (
                "Runtime.evaluate",
                value(json!({"x": 40.5, "y": 12, "covered": null})),
            ),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            // A new document replaced the marked one, and has loaded.
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", page()),
        ]);
        run(&script, &json!({"action": "click", "ref": "e7"})).unwrap();
        let calls = script.calls.borrow();
        assert!(
            calls[1].1["expression"]
                .as_str()
                .unwrap()
                .contains("get(7)")
        );
        assert_eq!(calls[3].1["type"], "mousePressed");
        assert_eq!(calls[3].1["x"], 40.5);
        assert_eq!(calls[3].1["clickCount"], 1);
        assert_eq!(calls[4].1["type"], "mouseReleased");
    }

    #[test]
    fn a_covered_or_stale_element_is_not_clicked() {
        let covered = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            (
                "Runtime.evaluate",
                value(json!({"x": 1, "y": 1, "covered": "div.modal"})),
            ),
        ]);
        let error = run(&covered, &json!({"action": "click", "ref": "e2"})).unwrap_err();
        assert!(
            error.starts_with("A click on the middle of e2 would reach div.modal, not e2."),
            "{error}"
        );
        assert!(
            !covered
                .methods()
                .contains(&"Input.dispatchMouseEvent".into())
        );
        let stale = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            ("Runtime.evaluate", value(json!({"missing": true}))),
        ]);
        assert!(
            run(&stale, &json!({"action": "click", "ref": "e2"}))
                .unwrap_err()
                .contains("new browser_snapshot")
        );
        assert!(
            run(
                &Script::new(vec![]),
                &json!({"action": "click", "ref": "button"})
            )
            .unwrap_err()
            .contains("not an element ref")
        );
    }

    #[test]
    fn typing_replaces_the_field_and_can_submit() {
        let script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            ("Runtime.evaluate", value(json!("ok"))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.insertText", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", page()),
        ]);
        run(
            &script,
            &json!({"action": "type", "ref": "e1", "text": "shoes", "submit": true}),
        )
        .unwrap();
        let calls = script.calls.borrow();
        // The document is marked before typing can send the page on.
        assert!(
            calls[0].1["expression"]
                .as_str()
                .unwrap()
                .starts_with("window.__xdBrowserMark")
        );
        assert_eq!(calls[2].1["commands"], json!(["selectAll"]));
        assert_eq!(calls[4].1, json!({"text": "shoes"}));
        assert_eq!(calls[5].1["type"], "keyDown");
        assert_eq!(calls[5].1["key"], "Enter");
        assert_eq!(calls[5].1["text"], "\r");
    }

    #[test]
    fn a_click_waits_for_a_navigation_still_waiting_for_its_server() {
        let script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            (
                "Runtime.evaluate",
                value(json!({"x": 5, "y": 5, "covered": null})),
            ),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            // The old document stays while the server takes its time, longer
            // than a click waits for a navigation to begin.
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            ("Runtime.evaluate", value(json!([true, "complete"]))),
            (
                "Runtime.evaluate",
                Err("Execution context was destroyed.".into()),
            ),
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", page()),
        ]);
        // Loading for the first nine looks after the click, then done.
        *script.loading.borrow_mut() = [true; 9].into();
        let started = Instant::now();
        let outcome = run(&script, &json!({"action": "click", "ref": "e1"})).unwrap();
        assert!(started.elapsed() > SETTLE_TIME);
        assert!(outcome.text.starts_with("Page: Shop"), "{}", outcome.text);
        assert!(script.answers.borrow().is_empty());
    }

    #[test]
    fn a_page_that_sends_the_browser_on_is_not_the_result() {
        let script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            // A new document, complete, while the page it sends the browser
            // to loads.
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", page()),
        ]);
        *script.loading.borrow_mut() = [true, true].into();
        let outcome = run(&script, &json!({"action": "press_key", "key": "Enter"})).unwrap();
        assert!(outcome.text.starts_with("Page: Shop"), "{}", outcome.text);
        assert!(script.answers.borrow().is_empty());
    }

    #[test]
    fn a_page_that_sends_the_browser_on_once_loaded_is_not_the_result() {
        let script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            // Complete and not loading, until it sends the browser on during
            // the pause that follows.
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", value(json!([false, "complete"]))),
            ("Runtime.evaluate", page()),
        ]);
        // Looked at before the pause, after it, then before and after again.
        *script.loading.borrow_mut() = [false, true, false, false].into();
        let outcome = run(&script, &json!({"action": "press_key", "key": "Enter"})).unwrap();
        assert!(outcome.text.starts_with("Page: Shop"), "{}", outcome.text);
        assert!(script.answers.borrow().is_empty());
        assert!(script.loading.borrow().is_empty());
    }

    #[test]
    fn appending_moves_to_the_end_of_the_field_first() {
        let mut script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            ("Runtime.evaluate", value(json!("ok"))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.insertText", Ok(json!({}))),
        ]);
        // The page stays as it was for the rest.
        script.fallback = Some(json!([true, "complete"]));
        let _ = run(
            &script,
            &json!({"action": "type", "ref": "e4", "text": ".org", "append": true}),
        );
        let calls = script.calls.borrow();
        assert_eq!(calls[2].1["type"], "rawKeyDown");
        assert_eq!(calls[2].1["commands"], json!(["moveToEndOfDocument"]));
        assert_eq!(calls[4].1, json!({"text": ".org"}));
    }

    #[test]
    fn a_command_past_its_deadline_does_nothing_more() {
        let script = Script::new(vec![]);
        assert_eq!(
            super::run(
                &script,
                &json!({"action": "press_key", "key": "Enter"}),
                Instant::now(),
                &AtomicBool::new(false)
            ),
            Err(OUT_OF_TIME.into())
        );
        assert_eq!(
            super::run(
                &script,
                &json!({"action": "press_key", "key": "Enter"}),
                Instant::now() + COMMAND_TIME,
                &AtomicBool::new(true)
            ),
            Err(CANCELLED.into())
        );
        assert!(script.calls.borrow().is_empty());
    }

    #[test]
    fn a_command_stopped_while_it_waits_lets_go_at_once() {
        let mut script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
        ]);
        script.fallback = Some(json!([true, "complete"]));
        // The page keeps loading, so the wait would run its full time.
        *script.loading.borrow_mut() = [true; 1000].into();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let stop = {
            let cancelled = cancelled.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                cancelled.store(true, Ordering::Release);
            })
        };
        let started = Instant::now();
        let outcome = super::run(
            &script,
            &json!({"action": "press_key", "key": "Enter"}),
            started + COMMAND_TIME,
            &cancelled,
        );
        stop.join().unwrap();
        assert_eq!(outcome, Err(CANCELLED.into()));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// Stops its command once `calls` DevTools calls have been made.
    struct StopAfter<'a> {
        script: &'a Script,
        calls: usize,
        cancelled: &'a AtomicBool,
    }

    impl Page for StopAfter<'_> {
        fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
            let answer = self.script.call(method, params, timeout);
            if self.script.calls.borrow().len() >= self.calls {
                self.cancelled.store(true, Ordering::Release);
            }
            answer
        }
    }

    #[test]
    fn a_command_stopped_while_a_button_is_down_lets_it_go() {
        let script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            (
                "Runtime.evaluate",
                value(json!({"x": 5, "y": 5, "covered": null})),
            ),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
            ("Input.dispatchMouseEvent", Ok(json!({}))),
        ]);
        let cancelled = AtomicBool::new(false);
        // Stopped as soon as the button goes down.
        let page = StopAfter {
            script: &script,
            calls: 4,
            cancelled: &cancelled,
        };
        let outcome = super::run(
            &page,
            &json!({"action": "click", "ref": "e1"}),
            Instant::now() + COMMAND_TIME,
            &cancelled,
        );
        assert_eq!(outcome, Err(CANCELLED.into()));
        let calls = script.calls.borrow();
        assert_eq!(calls[3].1["type"], "mousePressed");
        assert_eq!(calls[4].1["type"], "mouseReleased");
        assert_eq!(calls.len(), 5);
    }

    /// Stopped while its key press still waited to be sent, which never is.
    struct StoppedBeforeSending<'a> {
        script: &'a Script,
        cancelled: &'a AtomicBool,
        sent: Cell<bool>,
    }

    impl Page for StoppedBeforeSending<'_> {
        fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
            if method == "Input.dispatchKeyEvent" {
                self.cancelled.store(true, Ordering::Release);
                self.sent.set(false);
                return Err("The command was stopped.".into());
            }
            self.sent.set(true);
            self.script.call(method, params, timeout)
        }

        fn last_call_sent(&self) -> bool {
            self.sent.get()
        }
    }

    #[test]
    fn a_key_never_pressed_is_not_let_go_of() {
        let script = Script::new(vec![("Runtime.evaluate", value(json!(null)))]);
        let cancelled = AtomicBool::new(false);
        let page = StoppedBeforeSending {
            script: &script,
            cancelled: &cancelled,
            sent: Cell::new(true),
        };
        let outcome = super::run(
            &page,
            &json!({"action": "press_key", "key": "Enter"}),
            Instant::now() + COMMAND_TIME,
            &cancelled,
        );
        assert_eq!(outcome, Err("The command was stopped.".into()));
        // Only the mark: no keyUp followed the press that never went.
        assert_eq!(script.calls.borrow().len(), 1);
    }

    #[test]
    fn a_page_that_keeps_loading_is_not_taken_for_a_settled_one() {
        let mut script = Script::new(vec![
            ("Runtime.evaluate", value(json!(null))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
            ("Input.dispatchKeyEvent", Ok(json!({}))),
        ]);
        // The old document stays, and the page keeps loading, as it already
        // was before the key: a navigation could still be on its way.
        script.fallback = Some(json!([true, "complete"]));
        *script.loading.borrow_mut() = [true; 1000].into();
        let started = Instant::now();
        let outcome = super::run(
            &script,
            &json!({"action": "press_key", "key": "Enter"}),
            // Ends the wait well before the usual 15 seconds.
            started + SETTLE_TIME * 2,
            &AtomicBool::new(false),
        );
        // It waited past the time an unchanged page counts as settled, until
        // the deadline, which then refused the snapshot.
        assert!(started.elapsed() >= SETTLE_TIME * 2);
        assert_eq!(outcome, Err(OUT_OF_TIME.into()));
    }

    #[test]
    fn commands_on_one_page_take_turns() {
        let turns = Mutex::new(());
        let running = AtomicBool::new(false);
        let first = take_turn(&turns, Instant::now() + Duration::from_secs(1), &running).unwrap();
        assert!(
            take_turn(
                &turns,
                Instant::now() + Duration::from_millis(100),
                &running
            )
            .unwrap_err()
            .contains("busy")
        );
        assert_eq!(
            take_turn(
                &turns,
                Instant::now() + Duration::from_secs(1),
                &AtomicBool::new(true)
            )
            .unwrap_err(),
            CANCELLED
        );
        drop(first);
        assert!(take_turn(&turns, Instant::now(), &running).is_ok());
    }

    #[test]
    fn keys_are_named_or_single_characters() {
        assert_eq!(key("pagedown").unwrap().key_code, 34);
        assert_eq!(
            key("q").unwrap(),
            Key {
                key: "q".into(),
                code: "KeyQ".into(),
                key_code: 81,
                text: Some("q".into())
            }
        );
        assert!(key("Hyper").is_err());
    }

    #[test]
    fn evaluation_returns_json_or_the_exception() {
        let script = Script::new(vec![
            ("Runtime.evaluate", value(json!({"items": 3}))),
            (
                "Runtime.evaluate",
                Ok(json!({"result": {"type": "object"},
                    "exceptionDetails": {"text": "Uncaught", "exception":
                        {"description": "ReferenceError: nope is not defined"}}})),
            ),
            (
                "Runtime.evaluate",
                Ok(json!({"result": {"type": "undefined"}})),
            ),
        ]);
        let evaluate = |expression: &str| {
            run(
                &script,
                &json!({"action": "evaluate", "expression": expression}),
            )
        };
        assert_eq!(evaluate("x").unwrap().text, "{\n  \"items\": 3\n}");
        assert_eq!(
            evaluate("nope").unwrap_err(),
            "The script threw: ReferenceError: nope is not defined"
        );
        assert_eq!(evaluate("void 0").unwrap().text, "undefined");
    }

    #[test]
    fn a_full_page_screenshot_is_clipped_to_a_readable_height() {
        let script = Script::new(vec![
            (
                "Page.getLayoutMetrics",
                Ok(json!({"cssContentSize": {"width": 1200, "height": 20000}})),
            ),
            ("Page.captureScreenshot", Ok(json!({"data": "AAAA"}))),
            ("Runtime.evaluate", value(json!("http://localhost/"))),
        ]);
        let outcome = run(&script, &json!({"action": "screenshot", "full_page": true})).unwrap();
        assert_eq!(outcome.image, Some(("AAAA".into(), "image/jpeg")));
        assert!(outcome.text.contains("first 6000"));
        assert_eq!(script.calls.borrow()[1].1["clip"]["height"], 6000.0);
        assert_eq!(
            outcome.into_value()["image"],
            json!({"data": "AAAA", "mime_type": "image/jpeg"})
        );
    }
}
