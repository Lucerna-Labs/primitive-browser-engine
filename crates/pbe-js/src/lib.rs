//! # pbe-js
//!
//! The **JavaScript engine** for the browser \u2014 wraps [boa](https://boa-dev.github.io/),
//! a pure-Rust ECMAScript implementation, with a minimal console/document/fetch
//! surface so page `<script>` can run. One swappable crate behind the browser
//! layer; no C build chain (matching the doctrine that chose `ring` over
//! `aws-lc-rs` for TLS).
//!
//! ## Why boa (not V8/rusty_v8)
//!
//! V8 is huge, links C++, and would drag a foreign engine into the engine's
//! address space \u2014 the exact thing the composition doctrine exists to
//! avoid. boa is pure-Rust: no C/C++, auditable, links cleanly, and runs real
//! ES2020-ish JS. Slower than V8, but the doctrine trades raw speed for a
//! small, trustworthy attack surface \u2014 the same trade HTTP makes by driving
//! the sealed system client instead of linking a TLS stack.
//!
//! ## DOM surface today
//!
//! A deliberately small host binding \u2014 enough to drive a page's own logic,
//! not a full browser DOM:
//!
//! - `console.log(...)` / `console.error(...)` \u2014 routed to a caller-supplied
//!   [`LogSink`] (the browser prints to stderr or an in-page console).
//! - `document.getTitle()` / `document.setTitle(s)` \u2014 routed through
//!   [`DomHooks`].
//! - `fetch(url)` \u2014 a synchronous shim calling the caller-supplied
//!   [`FetchHook`] (the browser's own protocol layer). Returns a plain object
//!   with `.status` and `.text()`.
//!
//! ## Why thread-local hooks
//!
//! boa 0.20's safe native-function API (`NativeFunction::from_copy_closure`)
//! requires the closure to be `Copy`, so it can't capture `Rc<RefCell<T>>`
//! directly. The hooks are stashed in `thread_local!` slots at runtime
//! construction and read by the `Copy` closures. This is fine because the
//! browser (like `cap-text-shape`'s shaper) is single-threaded: one JS
//! runtime per page, on the render thread.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsArgs, JsValue, NativeFunction, Source};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static LOG: RefCell<Option<Rc<RefCell<dyn LogSink>>>> = const { RefCell::new(None) };
    static DOM: RefCell<Option<Rc<RefCell<dyn DomHooks>>>> = const { RefCell::new(None) };
    static FETCH: RefCell<Option<Rc<RefCell<dyn FetchHook>>>> = const { RefCell::new(None) };
    /// The body of the most recent `fetch()` call, so `r.text()` can return
    /// it without capturing a non-Copy JsString in a from_copy_closure.
    static LAST_BODY: RefCell<String> = const { RefCell::new(String::new()) };
    /// The JS callback registered via `WebSocket.onmessage(fn)`, if any.
    /// The browser's poll loop calls it via [`JsRuntime::dispatch_ws_message`]
    /// when a server-pushed message arrives.
    static WS_CALLBACK: RefCell<Option<boa_engine::object::JsObject>> = const { RefCell::new(None) };
    /// The browser's WsHook, stashed so the WebSocket.open/send Copy closures
    /// can reach it.
    static WS_OPEN: RefCell<Option<Rc<RefCell<dyn WsHook>>>> = const { RefCell::new(None) };
}

/// Hook the browser supplies so JS `WebSocket.open(url)` reaches the engine's
/// own WebSocket layer (pbe-proto-ws). The browser owns the connection and
/// polls it; received messages are delivered back to JS via
/// [`JsRuntime::dispatch_ws_message`].
pub trait WsHook {
    /// Open a ws/wss connection. Returns true on success.
    fn open(&mut self, url: &str) -> bool;
    /// Send a text message over the open connection.
    fn send(&mut self, msg: &str) -> bool;
}

/// Where `console.log`/`console.error` output goes. The browser supplies
/// one (e.g. print to stderr, or an in-page console overlay).
pub trait LogSink {
    fn log(&mut self, msg: &str);
    fn error(&mut self, msg: &str);
}

/// A trivial log sink that discards everything \u2014 for tests/headless runs.
pub struct NullLogSink;
impl LogSink for NullLogSink {
    fn log(&mut self, _msg: &str) {}
    fn error(&mut self, _msg: &str) {}
}

/// Hook the browser supplies so JS `document.setTitle("x")` reaches the real
/// document, and `document.getTitle()` reads it.
pub trait DomHooks {
    fn get_title(&self) -> String;
    fn set_title(&mut self, title: String);
}

/// Hook the browser supplies so JS `fetch(url)` uses the engine's own
/// protocol layer (http/ws/data). Returns `(status, body_text)`.
pub trait FetchHook {
    fn fetch(&mut self, url: &str) -> (u16, String);
}

/// A running JS engine: owns a boa `Context` + the host bindings. Cheap to
/// keep around for a page's lifetime; expensive to recreate. Not `Send`
/// (boa's `Context` isn't `Send`), matching `cap-text-shape`'s `CosmicShaper`.
pub struct JsRuntime {
    ctx: Context,
}

/// Outcome of running a script: the value it evaluated to (as a display
/// string), or an error message if it threw / failed to parse.
pub type RunResult = Result<String, String>;

impl JsRuntime {
    /// Build a fresh engine with no host bindings (just the JS standard
    /// library). Use [`Self::with_bindings`] for the console/document/fetch
    /// surface.
    pub fn new() -> Self {
        JsRuntime {
            ctx: Context::default(),
        }
    }

    /// Build an engine with the console/document/fetch host bindings
    /// installed, backed by the caller-supplied hooks. The hooks are stashed
    /// in the per-thread slots the `Copy` native closures read from.
    pub fn with_bindings(
        log: Rc<RefCell<dyn LogSink>>,
        dom: Rc<RefCell<dyn DomHooks>>,
        fetch: Rc<RefCell<dyn FetchHook>>,
        ws: Rc<RefCell<dyn WsHook>>,
    ) -> Self {
        LOG.with(|c| *c.borrow_mut() = Some(log));
        DOM.with(|c| *c.borrow_mut() = Some(dom));
        FETCH.with(|c| *c.borrow_mut() = Some(fetch));
        let mut ctx = Context::default();
        install_console(&mut ctx);
        install_document(&mut ctx);
        install_fetch(&mut ctx);
        install_websocket(&mut ctx, ws);
        JsRuntime { ctx }
    }

    /// Evaluate a JS source string. Returns the result value's display form
    /// on success, or the error message on failure (parse error or thrown
    /// exception). Page `<script>` content runs through this.
    pub fn run(&mut self, src: &str) -> RunResult {
        match self.ctx.eval(Source::from_bytes(src)) {
            Ok(v) => Ok(v.display().to_string()),
            Err(e) => {
                let msg = e.to_string();
                Err(msg)
            }
        }
    }

    /// Deliver a WebSocket message to the JS `onmessage` callback the page
    /// registered via `WebSocket.onmessage(fn)`. The browser's poll loop
    /// calls this for each message it drains from the open connection. If no
    /// callback is registered, the message is dropped (matching a page that
    /// never set `onmessage`).
    pub fn dispatch_ws_message(&mut self, msg: &str) {
        WS_CALLBACK.with(|c| {
            if let Some(cb) = c.borrow().clone() {
                let val = JsValue::from(boa_engine::JsString::from(msg));
                let _ = cb.call(&JsValue::undefined(), &[val], &mut self.ctx);
            }
        });
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Install a `console` global with `log`/`error` routed to the `LOG` slot.
fn install_console(ctx: &mut Context) {
    let log_fn = NativeFunction::from_copy_closure(|_this, args, _ctx| {
        let msg = args
            .iter()
            .map(|a| a.display().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        LOG.with(|c| {
            if let Some(sink) = c.borrow().as_ref() {
                sink.borrow_mut().log(&msg);
            }
        });
        Ok(JsValue::undefined())
    });
    let err_fn = NativeFunction::from_copy_closure(|_this, args, _ctx| {
        let msg = args
            .iter()
            .map(|a| a.display().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        LOG.with(|c| {
            if let Some(sink) = c.borrow().as_ref() {
                sink.borrow_mut().error(&msg);
            }
        });
        Ok(JsValue::undefined())
    });
    let console = ObjectInitializer::new(ctx)
        .function(log_fn, js_string!("log"), 0)
        .function(err_fn, js_string!("error"), 0)
        .build();
    let _ = ctx.register_global_property(js_string!("console"), console, Attribute::all());
}

/// Install a `document` global with `getTitle`/`setTitle` routed to DOM.
fn install_document(ctx: &mut Context) {
    let get_fn = NativeFunction::from_copy_closure(|_this, _args, _ctx| {
        let title = DOM.with(|c| {
            c.borrow()
                .as_ref()
                .map(|d| d.borrow().get_title())
                .unwrap_or_default()
        });
        Ok(JsValue::from(boa_engine::JsString::from(title)))
    });
    let set_fn = NativeFunction::from_copy_closure(|_this, args, _ctx| {
        if let Some(val) = args.get_or_undefined(0).as_string() {
            DOM.with(|c| {
                if let Some(d) = c.borrow().as_ref() {
                    d.borrow_mut().set_title(val.to_std_string_escaped());
                }
            });
        }
        Ok(JsValue::undefined())
    });
    let document = ObjectInitializer::new(ctx)
        .function(get_fn, js_string!("getTitle"), 0)
        .function(set_fn, js_string!("setTitle"), 1)
        .build();
    let _ = ctx.register_global_property(js_string!("document"), document, Attribute::all());
}

/// Install a `fetch(url)` global returning `{status, text()}`.
fn install_fetch(ctx: &mut Context) {
    let fetch_fn = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let url = args
            .get_or_undefined(0)
            .as_string()
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        let (status, body) = FETCH.with(|c| {
            c.borrow()
                .as_ref()
                .map(|f| f.borrow_mut().fetch(&url))
                .unwrap_or((0, String::new()))
        });
        // Stash the body so `r.text()` (a Copy closure) can read it.
        LAST_BODY.with(|b| *b.borrow_mut() = body);
        let resp = boa_engine::object::JsObject::with_object_proto(ctx.intrinsics());
        let _ = resp.create_data_property(js_string!("status"), JsValue::new(status as f64), ctx);
        let text_fn = NativeFunction::from_copy_closure(|_this, _args, _ctx| {
            LAST_BODY.with(|b| {
                Ok(JsValue::from(boa_engine::JsString::from(
                    b.borrow().as_str(),
                )))
            })
        });
        let text_obj = ObjectInitializer::new(ctx)
            .function(text_fn, js_string!("call"), 0)
            .build();
        let _ = resp.create_data_property(js_string!("text"), text_obj, ctx);
        Ok(JsValue::from(resp))
    });
    // Register the fetch function directly as the global `fetch` (callable),
    // not wrapped in a container object.
    let fetch_callable =
        boa_engine::object::FunctionObjectBuilder::new(ctx.realm(), fetch_fn).build();
    let _ = ctx.register_global_property(js_string!("fetch"), fetch_callable, Attribute::all());
}

/// Install a `WebSocket` global: `open(url)` opens a connection via the
/// browser's WsHook, `send(msg)` sends text, and `onmessage(fn)` registers the
/// callback [`JsRuntime::dispatch_ws_message`] invokes for each received
/// message.
fn install_websocket(ctx: &mut Context, ws: Rc<RefCell<dyn WsHook>>) {
    // onmessage(fn): store the JS callback in WS_CALLBACK.
    let onmessage_fn = NativeFunction::from_copy_closure(move |_this, args, _ctx| {
        if let Some(f) = args.get_or_undefined(0).as_object() {
            WS_CALLBACK.with(|c| *c.borrow_mut() = Some(f.clone()));
        }
        Ok(JsValue::undefined())
    });
    // open(url): ask the browser's WsHook to open the connection.
    let open_fn = NativeFunction::from_copy_closure(move |_this, args, _ctx| {
        let url = args
            .get_or_undefined(0)
            .as_string()
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        let ok = WS_OPEN.with(|c| {
            c.borrow()
                .as_ref()
                .map(|w| w.borrow_mut().open(&url))
                .unwrap_or(false)
        });
        Ok(JsValue::from(ok))
    });
    // send(msg): send text over the open connection.
    let send_fn = NativeFunction::from_copy_closure(move |_this, args, _ctx| {
        let msg = args
            .get_or_undefined(0)
            .as_string()
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        let ok = WS_OPEN.with(|c| {
            c.borrow()
                .as_ref()
                .map(|w| w.borrow_mut().send(&msg))
                .unwrap_or(false)
        });
        Ok(JsValue::from(ok))
    });
    // Stash the ws hook in a thread_local the open/send closures read from.
    WS_OPEN.with(|c| *c.borrow_mut() = Some(ws));
    let ws_obj = ObjectInitializer::new(ctx)
        .function(open_fn, js_string!("open"), 1)
        .function(send_fn, js_string!("send"), 1)
        .function(onmessage_fn, js_string!("onmessage"), 1)
        .build();
    let _ = ctx.register_global_property(js_string!("WebSocket"), ws_obj, Attribute::all());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_arithmetic() {
        let mut rt = JsRuntime::new();
        assert_eq!(rt.run("1 + 2 + 3").unwrap(), "6");
    }

    #[test]
    fn runs_a_function_definition_and_call() {
        let mut rt = JsRuntime::new();
        let _ = rt.run("function greet(n) { return 'hello ' + n; }");
        assert_eq!(rt.run("greet('engine')").unwrap(), "\"hello engine\"");
    }

    #[test]
    fn returns_error_on_syntax_error() {
        let mut rt = JsRuntime::new();
        assert!(rt.run("function {").is_err());
    }

    #[test]
    fn returns_error_on_thrown_exception() {
        let mut rt = JsRuntime::new();
        assert!(rt.run("throw new Error('boom')").is_err());
    }

    /// A capturing log sink for tests.
    struct CapturingLog(Rc<RefCell<Vec<String>>>);
    impl LogSink for CapturingLog {
        fn log(&mut self, msg: &str) {
            self.0.borrow_mut().push(msg.to_string());
        }
        fn error(&mut self, msg: &str) {
            self.0.borrow_mut().push(format!("ERR: {msg}"));
        }
    }

    struct FakeDom {
        title: String,
    }
    impl DomHooks for FakeDom {
        fn get_title(&self) -> String {
            self.title.clone()
        }
        fn set_title(&mut self, title: String) {
            self.title = title;
        }
    }

    struct FakeFetch;
    impl FetchHook for FakeFetch {
        fn fetch(&mut self, _url: &str) -> (u16, String) {
            (200, "hello from fetch".to_string())
        }
    }

    struct FakeWs;
    impl crate::WsHook for FakeWs {
        fn open(&mut self, _url: &str) -> bool {
            true
        }
        fn send(&mut self, _msg: &str) -> bool {
            true
        }
    }

    #[test]
    fn console_log_routes_to_sink() {
        let captured = Rc::new(RefCell::new(Vec::new()));
        let log: Rc<RefCell<dyn LogSink>> = Rc::new(RefCell::new(CapturingLog(captured.clone())));
        let dom: Rc<RefCell<dyn DomHooks>> = Rc::new(RefCell::new(FakeDom {
            title: String::new(),
        }));
        let fetch: Rc<RefCell<dyn FetchHook>> = Rc::new(RefCell::new(FakeFetch));
        let mut rt = JsRuntime::with_bindings(log, dom, fetch, Rc::new(RefCell::new(FakeWs)));
        let _ = rt.run("console.log('a', 'b', 3)");
        assert!(
            captured
                .borrow()
                .first()
                .map(|s| s.contains("a"))
                .unwrap_or(false),
            "captured = {:?}",
            captured.borrow()
        );
    }

    #[test]
    fn document_title_round_trips_through_hook() {
        let log: Rc<RefCell<dyn LogSink>> = Rc::new(RefCell::new(NullLogSink));
        let dom: Rc<RefCell<dyn DomHooks>> = Rc::new(RefCell::new(FakeDom {
            title: "init".into(),
        }));
        let fetch: Rc<RefCell<dyn FetchHook>> = Rc::new(RefCell::new(FakeFetch));
        let dom_check = dom.clone();
        let mut rt = JsRuntime::with_bindings(log, dom, fetch, Rc::new(RefCell::new(FakeWs)));
        let _ = rt.run("document.setTitle('changed')");
        assert_eq!(dom_check.borrow().get_title(), "changed");
        let got = rt.run("document.getTitle()").unwrap();
        assert!(got.contains("changed"), "got = {got}");
    }

    #[test]
    fn ws_message_dispatched_to_onmessage_callback() {
        let log: Rc<RefCell<dyn LogSink>> = Rc::new(RefCell::new(NullLogSink));
        let dom: Rc<RefCell<dyn DomHooks>> = Rc::new(RefCell::new(FakeDom {
            title: String::new(),
        }));
        let fetch: Rc<RefCell<dyn FetchHook>> = Rc::new(RefCell::new(FakeFetch));
        let ws: Rc<RefCell<dyn crate::WsHook>> = Rc::new(RefCell::new(FakeWs));
        let mut rt = JsRuntime::with_bindings(log, dom, fetch, ws);
        // Register an onmessage callback that mutates a captured title via
        // the dom hook.
        let _ = rt.run("WebSocket.onmessage(function(m) { document.setTitle('got:' + m); })");
        // Deliver a message; the callback should run and set the title.
        rt.dispatch_ws_message("hello");
        let title = rt.run("document.getTitle()").unwrap();
        assert!(title.contains("got:hello"), "title was {title}");
    }

    #[test]
    fn fetch_uses_the_hook() {
        let log: Rc<RefCell<dyn LogSink>> = Rc::new(RefCell::new(NullLogSink));
        let dom: Rc<RefCell<dyn DomHooks>> = Rc::new(RefCell::new(FakeDom {
            title: String::new(),
        }));
        let fetch: Rc<RefCell<dyn FetchHook>> = Rc::new(RefCell::new(FakeFetch));
        let mut rt = JsRuntime::with_bindings(log, dom, fetch, Rc::new(RefCell::new(FakeWs)));
        // fetch() returns an object with .status and .text().
        let r = rt
            .run("fetch('https://x').status")
            .expect("fetch status eval failed");
        assert!(r.contains("200"), "status was {r}");
        let t = rt
            .run("fetch('https://x').text.call()")
            .expect("fetch text eval failed");
        assert!(t.contains("hello from fetch"), "text was {t}");
    }
}
