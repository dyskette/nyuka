//! The wasmtime linker bindings.
//!
//! A thin translation layer: read arguments out of guest memory, call the
//! module functions, put results into the resource table. All the semantics
//! live in the sibling modules, which is why those are testable without a
//! guest and this file has almost no logic of its own.
//!
//! Two conventions are easy to get wrong here, and both fail silently:
//!
//! - The buffer-filling imports return [`READ_OK`](crate::error::READ_OK),
//!   **not** a byte count.
//! - `defaults::set` receives a **pointer into guest memory**, not a handle,
//!   even though the parameter is an `i32` like every handle is.

use std::rc::Rc;

use wasmtime::{Caller, Extern, Linker, Memory};

use crate::error::{READ_OK, aidoku, html as html_err, net as net_err};
use crate::imports::{date, html, net};
use crate::resource::{Request, Resource};
use crate::state::HostState;

type Host<'a> = Caller<'a, HostState>;

fn memory(caller: &mut Host<'_>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
}

/// Reads a UTF-8 string out of guest memory.
fn read_str(caller: &mut Host<'_>, ptr: i32, len: i32) -> Option<String> {
    if len < 0 {
        return None;
    }
    let m = memory(caller)?;
    let mut buf = vec![0u8; len as usize];
    m.read(&mut *caller, ptr as usize, &mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Writes bytes into guest memory, truncating to the space offered.
fn write_bytes(caller: &mut Host<'_>, ptr: i32, len: i32, bytes: &[u8]) -> bool {
    let Some(m) = memory(caller) else {
        return false;
    };
    let n = bytes.len().min(len.max(0) as usize);
    m.write(&mut *caller, ptr as usize, &bytes[..n]).is_ok()
}

/// Reads a guest-encoded `[len, capacity, payload]` block.
///
/// `defaults::set` is the one import in the required surface that hands over a
/// pointer rather than a handle, which is easy to mistake because both are
/// `i32`.
fn read_encoded(caller: &mut Host<'_>, ptr: i32) -> Option<Vec<u8>> {
    if ptr <= 0 {
        return None;
    }
    let m = memory(caller)?;
    let mut header = [0u8; 8];
    m.read(&mut *caller, ptr as usize, &mut header).ok()?;
    let len = i32::from_le_bytes(header[0..4].try_into().ok()?);
    if len < 8 {
        return None;
    }
    let mut payload = vec![0u8; (len - 8) as usize];
    m.read(&mut *caller, (ptr + 8) as usize, &mut payload)
        .ok()?;
    Some(payload)
}

/// Reads `len` little-endian `i32`s from guest memory.
fn read_i32s(caller: &mut Host<'_>, ptr: i32, len: i32) -> Option<Vec<i32>> {
    if len < 0 || ptr < 0 {
        return None;
    }
    let count = len as usize;
    let bytes = count.checked_mul(4)?;
    let m = memory(caller)?;
    let mut buf = vec![0u8; bytes];
    m.read(&mut *caller, ptr as usize, &mut buf).ok()?;
    Some(
        buf.as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c))
            .collect(),
    )
}

/// Writes `i32`s back over the array the guest passed in.
fn write_i32s(caller: &mut Host<'_>, ptr: i32, values: &[i32]) -> bool {
    let Some(m) = memory(caller) else {
        return false;
    };
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    m.write(&mut *caller, ptr as usize, &bytes).is_ok()
}

/// Stores bytes and returns the handle, for imports that answer with a string.
fn put_bytes(caller: &mut Host<'_>, bytes: Vec<u8>) -> i32 {
    caller.data_mut().table.insert(Resource::Buffer(bytes))
}

/// Every `(module, function)` this host defines.
///
/// `package::load` refuses a source importing anything outside this set: a
/// capability being provided does not mean every function in it is (ADR-0004).
/// `every_registered_import_is_listed` keeps it in step with `register`.
pub const PROVIDED: &[(&str, &str)] = &[
    // env
    ("env", "abort"),
    ("env", "print"),
    ("env", "send_partial_result"),
    ("env", "sleep"),
    // std
    ("std", "abort"),
    ("std", "buffer_len"),
    ("std", "current_date"),
    ("std", "destroy"),
    ("std", "parse_date"),
    ("std", "print"),
    ("std", "read_buffer"),
    ("std", "utc_offset"),
    // defaults
    ("defaults", "get"),
    ("defaults", "set"),
    // net
    ("net", "data_len"),
    ("net", "get_header"),
    ("net", "get_status_code"),
    ("net", "get_url"),
    ("net", "html"),
    ("net", "init"),
    ("net", "read_data"),
    ("net", "send"),
    ("net", "send_all"),
    ("net", "set_body"),
    ("net", "set_header"),
    ("net", "set_rate_limit"),
    ("net", "set_timeout"),
    ("net", "set_url"),
    // html
    ("html", "add_class"),
    ("html", "append"),
    ("html", "attr"),
    ("html", "base_uri"),
    ("html", "child_nodes"),
    ("html", "children"),
    ("html", "class_name"),
    ("html", "escape"),
    ("html", "data"),
    ("html", "first"),
    ("html", "get"),
    ("html", "has_attr"),
    ("html", "has_class"),
    ("html", "html"),
    ("html", "id"),
    ("html", "last"),
    ("html", "kind"),
    ("html", "next"),
    ("html", "outer_html"),
    ("html", "own_text"),
    ("html", "parent"),
    ("html", "parse"),
    ("html", "parse_fragment"),
    ("html", "prepend"),
    ("html", "previous"),
    ("html", "remove"),
    ("html", "remove_attr"),
    ("html", "remove_class"),
    ("html", "select"),
    ("html", "select_first"),
    ("html", "set_attr"),
    ("html", "set_html"),
    ("html", "set_text"),
    ("html", "siblings"),
    ("html", "size"),
    ("html", "tag_name"),
    ("html", "text"),
    ("html", "unescape"),
    ("html", "untrimmed_text"),
];

/// The longest `env::sleep` this host honours.
const MAX_SLEEP_SECONDS: i32 = 30;

/// Registers every host import the required surface needs.
pub fn register(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    register_env(linker)?;
    register_std(linker)?;
    register_defaults(linker)?;
    register_net(linker)?;
    register_html(linker)?;
    Ok(())
}

/// `env` — functions whose guest-side `extern` block names no import module,
/// so they land in the default one. A host registering only the named modules
/// fails to instantiate.
fn register_env(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap("env", "print", |mut c: Host<'_>, p: i32, l: i32| {
        if let Some(s) = read_str(&mut c, p, l) {
            c.data_mut().logs.push(s);
        }
    })?;
    linker.func_wrap("env", "abort", |_c: Host<'_>| {})?;
    linker.func_wrap("env", "sleep", |_c: Host<'_>, seconds: i32| {
        // Bounded: epoch interruption cannot preempt a host call, so an
        // unbounded sleep here would pin a worker for as long as the guest
        // asked. Sources use this to back off, which fits well inside the cap.
        let seconds = seconds.clamp(0, MAX_SLEEP_SECONDS);
        std::thread::sleep(std::time::Duration::from_secs(seconds as u64));
    })?;
    linker.func_wrap("env", "send_partial_result", |mut c: Host<'_>, ptr: i32| {
        // Sources stream progress through this; it maps onto job.progress.
        if let Some(bytes) = read_encoded(&mut c, ptr) {
            c.data_mut().partial_results.push(bytes);
        }
    })?;
    Ok(())
}

fn register_std(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap("std", "destroy", |mut c: Host<'_>, rid: i32| {
        c.data_mut().table.remove(rid);
    })?;
    linker.func_wrap("std", "buffer_len", |c: Host<'_>, rid: i32| -> i32 {
        match c.data().table.buffer(rid) {
            Some(b) => b.len() as i32,
            None => aidoku::MESSAGE,
        }
    })?;
    linker.func_wrap(
        "std",
        "read_buffer",
        |mut c: Host<'_>, rid: i32, ptr: i32, len: i32| -> i32 {
            let Some(bytes) = c.data().table.buffer(rid).map(<[u8]>::to_vec) else {
                return aidoku::MESSAGE;
            };
            // Zero on success, not the byte count: the guest treats any
            // non-zero value as an error.
            if write_bytes(&mut c, ptr, len, &bytes) {
                READ_OK
            } else {
                aidoku::MESSAGE
            }
        },
    )?;
    linker.func_wrap("std", "current_date", |_c: Host<'_>| -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    })?;
    linker.func_wrap("std", "utc_offset", |_c: Host<'_>| -> i64 { 0 })?;
    linker.func_wrap(
        "std",
        "parse_date",
        #[allow(clippy::too_many_arguments)]
        |mut c: Host<'_>,
         sp: i32,
         sl: i32,
         fp: i32,
         fl: i32,
         _lp: i32,
         _ll: i32,
         tp: i32,
         tl: i32|
         -> f64 {
            let date = read_str(&mut c, sp, sl).unwrap_or_default();
            let pattern = read_str(&mut c, fp, fl).unwrap_or_default();
            let timezone = read_str(&mut c, tp, tl).unwrap_or_default();
            match date::parse(&date, &pattern, &timezone) {
                Some(ts) => ts as f64,
                // Negative means error. Zero would be 1970, which the guest
                // cannot tell from a real date.
                None => aidoku::MESSAGE as f64,
            }
        },
    )?;
    linker.func_wrap("std", "print", |mut c: Host<'_>, p: i32, l: i32| {
        if let Some(s) = read_str(&mut c, p, l) {
            c.data_mut().logs.push(s);
        }
    })?;
    linker.func_wrap("std", "abort", |_c: Host<'_>| {})?;
    Ok(())
}

fn register_defaults(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        "defaults",
        "get",
        |mut c: Host<'_>, p: i32, l: i32| -> i32 {
            let Some(key) = read_str(&mut c, p, l) else {
                return aidoku::MESSAGE;
            };
            match c.data().defaults.get(&key) {
                // Unset is a miss, not an error.
                None => aidoku::MESSAGE,
                Some(bytes) => put_bytes(&mut c, bytes),
            }
        },
    )?;
    linker.func_wrap(
        "defaults",
        "set",
        |mut c: Host<'_>, p: i32, l: i32, _kind: i32, value: i32| -> i32 {
            let Some(key) = read_str(&mut c, p, l) else {
                return aidoku::MESSAGE;
            };
            // A null pointer is DefaultValue::Null.
            if value == 0 {
                c.data().defaults.remove(&key);
                return 0;
            }
            let Some(bytes) = read_encoded(&mut c, value) else {
                return aidoku::MESSAGE;
            };
            c.data().defaults.set(&key, bytes);
            0
        },
    )?;
    Ok(())
}

fn register_net(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap("net", "init", |mut c: Host<'_>, method: i32| -> i32 {
        c.data_mut().table.insert(Resource::Request(Request {
            method,
            ..Default::default()
        }))
    })?;
    linker.func_wrap(
        "net",
        "set_url",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(url) = read_str(&mut c, p, l) else {
                return net_err::INVALID_STRING;
            };
            match c.data_mut().table.request_mut(rid) {
                Some(r) => {
                    r.url = Some(url);
                    0
                }
                None => net_err::INVALID_DESCRIPTOR,
            }
        },
    )?;
    linker.func_wrap(
        "net",
        "set_header",
        |mut c: Host<'_>, rid: i32, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
            let (Some(k), Some(v)) = (read_str(&mut c, kp, kl), read_str(&mut c, vp, vl)) else {
                return net_err::INVALID_STRING;
            };
            match c.data_mut().table.request_mut(rid) {
                Some(r) => {
                    r.headers.push((k, v));
                    0
                }
                None => net_err::INVALID_DESCRIPTOR,
            }
        },
    )?;
    linker.func_wrap(
        "net",
        "set_body",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(body) = read_str(&mut c, p, l) else {
                return net_err::INVALID_STRING;
            };
            match c.data_mut().table.request_mut(rid) {
                Some(r) => {
                    r.body = Some(body.into_bytes());
                    0
                }
                None => net_err::INVALID_DESCRIPTOR,
            }
        },
    )?;
    linker.func_wrap(
        "net",
        "set_timeout",
        |_c: Host<'_>, _rid: i32, _seconds: f64| -> i32 {
            // The client's timeout is the operator's, not the source's: a
            // source must not be able to hold a worker open indefinitely.
            0
        },
    )?;
    linker.func_wrap(
        "net",
        "set_rate_limit",
        |mut c: Host<'_>, permits: i32, period: i32, unit: i32| {
            c.data_mut().declared_rate_limit = net::RateLimit::from_wire(permits, period, unit);
        },
    )?;
    linker.func_wrap("net", "send", |mut c: Host<'_>, rid: i32| -> i32 {
        let Some(request) = c.data().table.request(rid).cloned() else {
            return net_err::INVALID_DESCRIPTOR;
        };
        match net::send(&c.data().client, &request) {
            Ok(response) => {
                if let Some(r) = c.data_mut().table.request_mut(rid) {
                    r.response = Some(response);
                }
                0
            }
            Err(code) => code,
        }
    })?;
    linker.func_wrap(
        "net",
        "send_all",
        |mut c: Host<'_>, ptr: i32, len: i32| -> i32 {
            let Some(rids) = read_i32s(&mut c, ptr, len) else {
                return net_err::INVALID_DESCRIPTOR;
            };

            let requests: Option<Vec<_>> = rids
                .iter()
                .map(|rid| c.data().table.request(*rid).cloned())
                .collect();
            let Some(requests) = requests else {
                return net_err::INVALID_DESCRIPTOR;
            };

            let results = net::send_all(&c.data().client, &requests);

            // The guest reads per-request outcomes back out of the array it
            // passed, by index, and treats a non-zero return as "read them".
            let mut codes = rids.clone();
            let mut failure = 0;
            for (index, result) in results.into_iter().enumerate() {
                match result {
                    Ok(response) => {
                        if let Some(r) = c.data_mut().table.request_mut(rids[index]) {
                            r.response = Some(response);
                        }
                    }
                    Err(code) => {
                        codes[index] = code;
                        if failure == 0 {
                            failure = code;
                        }
                    }
                }
            }

            if failure != 0 && !write_i32s(&mut c, ptr, &codes) {
                return net_err::FAILED;
            }
            failure
        },
    )?;
    linker.func_wrap("net", "data_len", |c: Host<'_>, rid: i32| -> i32 {
        match c
            .data()
            .table
            .request(rid)
            .and_then(|r| r.response.as_ref())
        {
            Some(resp) => resp.body.len() as i32,
            None => net_err::NOT_SENT,
        }
    })?;
    linker.func_wrap(
        "net",
        "read_data",
        |mut c: Host<'_>, rid: i32, ptr: i32, len: i32| -> i32 {
            let Some(bytes) = c
                .data()
                .table
                .request(rid)
                .and_then(|r| r.response.as_ref())
                .map(|resp| resp.body.clone())
            else {
                return net_err::NOT_SENT;
            };
            if write_bytes(&mut c, ptr, len, &bytes) {
                READ_OK
            } else {
                net_err::FAILED
            }
        },
    )?;
    linker.func_wrap("net", "get_status_code", |c: Host<'_>, rid: i32| -> i32 {
        match c
            .data()
            .table
            .request(rid)
            .and_then(|r| r.response.as_ref())
        {
            Some(resp) => resp.status as i32,
            None => net_err::NOT_SENT,
        }
    })?;
    linker.func_wrap("net", "get_url", |mut c: Host<'_>, rid: i32| -> i32 {
        let url = c
            .data()
            .table
            .request(rid)
            .and_then(|r| r.response.as_ref().and_then(|resp| resp.final_url.clone()));
        match url {
            Some(u) => put_bytes(&mut c, u.into_bytes()),
            None => net_err::NOT_SENT,
        }
    })?;
    linker.func_wrap(
        "net",
        "get_header",
        |mut c: Host<'_>, rid: i32, kp: i32, kl: i32| -> i32 {
            let Some(key) = read_str(&mut c, kp, kl) else {
                return net_err::INVALID_STRING;
            };
            let found = c
                .data()
                .table
                .request(rid)
                .and_then(|r| r.response.as_ref())
                .and_then(|resp| {
                    resp.headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(&key))
                        .map(|(_, v)| v.clone())
                });
            match found {
                Some(v) => put_bytes(&mut c, v.into_bytes()),
                None => net_err::NOT_SENT,
            }
        },
    )?;
    linker.func_wrap("net", "html", |mut c: Host<'_>, rid: i32| -> i32 {
        let Some((body, base)) = c
            .data()
            .table
            .request(rid)
            .and_then(|r| r.response.as_ref())
            .map(|resp| {
                (
                    String::from_utf8_lossy(&resp.body).into_owned(),
                    // The URL the response came from, after redirects — the
                    // base every `abs:` attribute resolves against.
                    resp.final_url.clone(),
                )
            })
        else {
            return net_err::NOT_SENT;
        };
        let parsed = Rc::new(html::parse(&body, base.as_deref(), false));
        let root = html::root(&parsed);
        c.data_mut().table.insert(Resource::Node {
            html: parsed,
            id: root,
        })
    })?;
    Ok(())
}

/// Resolves a node handle, or returns the descriptor error.
macro_rules! node {
    ($c:expr, $rid:expr) => {
        match $c.data().table.node($rid) {
            Some((html, id)) => (html.clone(), id),
            None => return html_err::INVALID_DESCRIPTOR,
        }
    };
}

fn register_html(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        "html",
        "parse",
        |mut c: Host<'_>, p: i32, l: i32, bp: i32, bl: i32| -> i32 {
            parse_into(&mut c, p, l, bp, bl, false)
        },
    )?;
    linker.func_wrap(
        "html",
        "parse_fragment",
        |mut c: Host<'_>, p: i32, l: i32, bp: i32, bl: i32| -> i32 {
            parse_into(&mut c, p, l, bp, bl, true)
        },
    )?;
    linker.func_wrap(
        "html",
        "select",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(sel) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            match html::select(&doc, id, &sel) {
                Ok(ids) => c.data_mut().table.insert(Resource::NodeList {
                    html: doc,
                    ids: Rc::new(ids),
                }),
                Err(code) => code,
            }
        },
    )?;
    linker.func_wrap(
        "html",
        "select_first",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(sel) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            match html::select_first(&doc, id, &sel) {
                Ok(found) => c.data_mut().table.insert(Resource::Node {
                    html: doc,
                    id: found,
                }),
                Err(code) => code,
            }
        },
    )?;
    linker.func_wrap("html", "size", |c: Host<'_>, rid: i32| -> i32 {
        match c.data().table.get(rid) {
            Some(Resource::NodeList { ids, .. }) => ids.len() as i32,
            Some(Resource::Node { .. }) => 1,
            _ => html_err::INVALID_DESCRIPTOR,
        }
    })?;
    linker.func_wrap(
        "html",
        "get",
        |mut c: Host<'_>, rid: i32, idx: i32| -> i32 {
            let picked = match c.data().table.get(rid) {
                Some(Resource::NodeList { html, ids }) => {
                    ids.get(idx.max(0) as usize).map(|id| (html.clone(), *id))
                }
                _ => return html_err::INVALID_DESCRIPTOR,
            };
            match picked {
                Some((html, id)) => c.data_mut().table.insert(Resource::Node { html, id }),
                None => html_err::NO_RESULT,
            }
        },
    )?;
    linker.func_wrap(
        "html",
        "attr",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(key) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            match html::attr(&doc, id, &key) {
                Ok(v) => put_bytes(&mut c, v.into_bytes()),
                Err(code) => code,
            }
        },
    )?;
    // The text-returning accessors differ only in which function they call.
    macro_rules! text_accessor {
        ($name:literal, $f:path) => {
            linker.func_wrap("html", $name, |mut c: Host<'_>, rid: i32| -> i32 {
                let (doc, id) = node!(c, rid);
                let value = $f(&doc, id);
                put_bytes(&mut c, value.into_bytes())
            })?;
        };
    }
    text_accessor!("text", html::text);
    text_accessor!("own_text", html::own_text);
    text_accessor!("untrimmed_text", html::untrimmed_text);
    text_accessor!("html", html::outer_html);
    text_accessor!("outer_html", html::outer_html);

    // Accessors returning an optional string.
    macro_rules! opt_string_accessor {
        ($name:literal, $f:path) => {
            linker.func_wrap("html", $name, |mut c: Host<'_>, rid: i32| -> i32 {
                let (doc, id) = node!(c, rid);
                match $f(&doc, id) {
                    Some(v) => put_bytes(&mut c, v.into_bytes()),
                    None => html_err::NO_RESULT,
                }
            })?;
        };
    }
    opt_string_accessor!("data", html::data);
    opt_string_accessor!("tag_name", html::tag_name);
    opt_string_accessor!("id", html::element_id);
    opt_string_accessor!("class_name", html::class_name);

    // A list answers `ElementList` before the node table is consulted: it has
    // many ids and `node!` would reject it.
    linker.func_wrap("html", "kind", |c: Host<'_>, rid: i32| -> i32 {
        match c.data().table.get(rid) {
            Some(Resource::NodeList { .. }) => html::kind::ELEMENT_LIST,
            Some(Resource::Node { html: doc, id }) => html::node_kind(doc, *id),
            _ => html_err::INVALID_DESCRIPTOR,
        }
    })?;

    // Traversal, each returning a node handle or a miss.
    macro_rules! node_accessor {
        ($name:literal, $f:path) => {
            linker.func_wrap("html", $name, |mut c: Host<'_>, rid: i32| -> i32 {
                let (doc, id) = node!(c, rid);
                match $f(&doc, id) {
                    Some(found) => c.data_mut().table.insert(Resource::Node {
                        html: doc,
                        id: found,
                    }),
                    None => html_err::NO_RESULT,
                }
            })?;
        };
    }
    node_accessor!("parent", html::parent);
    node_accessor!("next", html::next_sibling);
    node_accessor!("previous", html::prev_sibling);
    node_accessor!("last", html::last_child);

    // `first` is the first entry of a selection, or the first element child of
    // a node.
    linker.func_wrap("html", "first", |mut c: Host<'_>, rid: i32| -> i32 {
        let picked = match c.data().table.get(rid) {
            Some(Resource::NodeList { html, ids }) => ids.first().map(|id| (html.clone(), *id)),
            Some(Resource::Node { html, id }) => {
                let html = html.clone();
                html::first_child(&html, *id).map(|found| (html, found))
            }
            _ => return html_err::INVALID_DESCRIPTOR,
        };
        match picked {
            Some((html, id)) => c.data_mut().table.insert(Resource::Node { html, id }),
            None => html_err::NO_RESULT,
        }
    })?;

    // Node lists.
    macro_rules! list_accessor {
        ($name:literal, $f:path) => {
            linker.func_wrap("html", $name, |mut c: Host<'_>, rid: i32| -> i32 {
                let (doc, id) = node!(c, rid);
                let ids = $f(&doc, id);
                c.data_mut().table.insert(Resource::NodeList {
                    html: doc,
                    ids: Rc::new(ids),
                })
            })?;
        };
    }
    list_accessor!("children", html::children);
    list_accessor!("child_nodes", html::children);
    list_accessor!("siblings", html::siblings);

    // Predicates.
    linker.func_wrap(
        "html",
        "has_attr",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(name) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            html::has_attr(&doc, id, &name) as i32
        },
    )?;
    linker.func_wrap(
        "html",
        "has_class",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(class) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            html::has_class(&doc, id, &class) as i32
        },
    )?;

    // Mutation taking one string argument.
    macro_rules! mutate_with_string {
        ($name:literal, $f:path) => {
            linker.func_wrap(
                "html",
                $name,
                |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
                    let Some(arg) = read_str(&mut c, p, l) else {
                        return html_err::INVALID_STRING;
                    };
                    let (doc, id) = node!(c, rid);
                    $f(&doc, id, &arg);
                    0
                },
            )?;
        };
    }
    mutate_with_string!("add_class", html::add_class);
    mutate_with_string!("remove_class", html::remove_class);
    mutate_with_string!("remove_attr", html::remove_attr);
    mutate_with_string!("append", html::append);
    mutate_with_string!("prepend", html::prepend);

    linker.func_wrap(
        "html",
        "set_attr",
        |mut c: Host<'_>, rid: i32, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
            let (Some(k), Some(v)) = (read_str(&mut c, kp, kl), read_str(&mut c, vp, vl)) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            html::set_attr(&doc, id, &k, &v);
            0
        },
    )?;
    linker.func_wrap("html", "remove", |c: Host<'_>, rid: i32| -> i32 {
        // Detaches the node from the shared document; the table is untouched,
        // so the handle stays valid and simply refers to a detached subtree.
        let (doc, id) = node!(c, rid);
        html::remove(&doc, id);
        0
    })?;

    // Entity helpers, which take text rather than a node.
    linker.func_wrap("html", "escape", |mut c: Host<'_>, p: i32, l: i32| -> i32 {
        let Some(text) = read_str(&mut c, p, l) else {
            return html_err::INVALID_STRING;
        };
        let escaped = html::escape(&text);
        put_bytes(&mut c, escaped.into_bytes())
    })?;
    linker.func_wrap(
        "html",
        "unescape",
        |mut c: Host<'_>, p: i32, l: i32| -> i32 {
            let Some(text) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let plain = html::unescape(&text);
            put_bytes(&mut c, plain.into_bytes())
        },
    )?;

    linker.func_wrap("html", "base_uri", |mut c: Host<'_>, rid: i32| -> i32 {
        let (doc, id) = node!(c, rid);
        match html::base_uri(&doc, id) {
            Some(base) => put_bytes(&mut c, base.into_bytes()),
            None => html_err::NO_RESULT,
        }
    })?;
    linker.func_wrap(
        "html",
        "set_text",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(text) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            html::set_text(&doc, id, &text);
            0
        },
    )?;
    linker.func_wrap(
        "html",
        "set_html",
        |mut c: Host<'_>, rid: i32, p: i32, l: i32| -> i32 {
            let Some(markup) = read_str(&mut c, p, l) else {
                return html_err::INVALID_STRING;
            };
            let (doc, id) = node!(c, rid);
            html::set_html(&doc, id, &markup);
            0
        },
    )?;
    Ok(())
}

fn parse_into(
    c: &mut Host<'_>,
    ptr: i32,
    len: i32,
    base_ptr: i32,
    base_len: i32,
    fragment: bool,
) -> i32 {
    let Some(markup) = read_str(c, ptr, len) else {
        return html_err::INVALID_STRING;
    };
    let base = read_str(c, base_ptr, base_len).filter(|b| !b.is_empty());
    let parsed = Rc::new(html::parse(&markup, base.as_deref(), fragment));
    let root = html::root(&parsed);
    c.data_mut().table.insert(Resource::Node {
        html: parsed,
        id: root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Defaults;

    struct NoDefaults;

    impl Defaults for NoDefaults {
        fn get(&self, _key: &str) -> Option<Vec<u8>> {
            None
        }
        fn set(&self, _key: &str, _value: Vec<u8>) {}
        fn remove(&self, _key: &str) {}
    }

    /// Exactly what `register` defines: a name listed but not registered lets
    /// a source install and trap, and one registered but not listed refuses a
    /// source this host can run. Read from a real linker, so any `func_wrap`
    /// counts.
    #[test]
    fn every_registered_import_is_listed() {
        let engine = wasmtime::Engine::default();
        let mut linker: Linker<HostState> = Linker::new(&engine);
        register(&mut linker).expect("the host imports register");

        let mut store = wasmtime::Store::new(
            &engine,
            HostState::new(
                std::sync::Arc::new(NoDefaults),
                reqwest::blocking::Client::new(),
            ),
        );

        let mut registered: Vec<(String, String)> = linker
            .iter(&mut store)
            .map(|(module, name, _)| (module.to_string(), name.to_string()))
            .collect();
        registered.sort();

        let mut listed: Vec<(String, String)> = PROVIDED
            .iter()
            .map(|(m, n)| ((*m).to_string(), (*n).to_string()))
            .collect();
        listed.sort();

        assert_eq!(
            registered, listed,
            "PROVIDED and `register` disagree; the left side is what the linker defines"
        );
    }

    /// `net::get_image` answers a canvas `ImageRef`, which is tier 3, so it
    /// stays unprovided and is refused by capability instead.
    #[test]
    fn get_image_is_not_claimed() {
        assert!(!PROVIDED.contains(&("net", "get_image")));
    }

    /// ADR-0004 commits this project to tier 1 in full, against the pinned
    /// aidoku-rs commit. The snapshot is that surface; regenerate it with
    /// `cargo xtask abi-surface` after re-pinning.
    #[test]
    fn the_whole_pinned_tier_one_surface_is_provided() {
        let snapshot = include_str!("../../abi/tier1-surface.txt");

        let missing: Vec<&str> = snapshot
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            // The one tier-3 dependency inside a tier-1 module: it answers a
            // canvas `ImageRef`, and is refused by capability.
            .filter(|line| *line != "net::get_image")
            .filter(|line| {
                let (module, name) = line.split_once("::").expect("module::name");
                !PROVIDED.contains(&(module, name))
            })
            .collect();

        assert!(
            missing.is_empty(),
            "the host does not provide the pinned tier-1 surface: {missing:?}"
        );
    }
}
