//! ADR-0004 spike: load a real `.aix` and run a source through hand-written
//! host imports, to establish that the Aidoku ABI is implementable in Rust
//! before the rest of the runtime is built on that assumption.
//!
//! Throwaway by design. It reports the ABI it finds rather than assuming one,
//! and it uses `scraper` (read-only) for HTML, which is *not* a candidate for
//! the real `html` module — that one mutates.
//!
//! Usage:
//!   cargo run -p nyuka-aidoku-runtime --example spike -- <path.aix> [query]

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use wasmtime::{Caller, Engine, Extern, Linker, Memory, Module, Store, Val, ValType};

// FFIResult: non-negative is success (a resource id or a value), negative is
// an error code. Taken from aidoku-rs `AidokuError::error_code`.
const ERR_MESSAGE: i32 = -1;
const ERR_UNIMPLEMENTED: i32 = -2;
const ERR_REQUEST: i32 = -3;

// The `html` module has its OWN error code space (aidoku-rs `HtmlError::from`),
// which is NOT the AidokuError space above. Returning the wrong one makes a
// missing element look like a malformed selector, and the source abandons the
// entry instead of skipping a field.
const HTML_INVALID_DESCRIPTOR: i32 = -1;
const HTML_INVALID_QUERY: i32 = -4;
const HTML_NO_RESULT: i32 = -5;

/// Host-owned objects the guest refers to by handle.
enum Res {
    /// Bytes the guest reads back via `std::buffer_len` + `std::read_buffer`.
    Buffer(Vec<u8>),
    Request(Req),
    /// A parsed document or element. Held as owned HTML so handles carry no
    /// lifetimes; the real implementation should keep a tree and node ids.
    ///
    /// The base URI travels with the node: `attr("abs:href")` resolves against
    /// it, and it must survive select -> get -> nested select_first, because
    /// that is the path by which a source obtains absolute cover and series
    /// URLs.
    Node {
        html: Rc<String>,
        base: Option<Rc<String>>,
    },
    NodeList {
        items: Rc<Vec<String>>,
        base: Option<Rc<String>>,
    },
}

#[derive(Default)]
struct Req {
    method: i32,
    url: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
}

#[derive(Default)]
struct State {
    table: HashMap<i32, Res>,
    next_id: i32,
    /// Every host call, in order — the point of the spike.
    calls: Vec<String>,
    /// Imports the guest needed that are not implemented here.
    missing: Vec<String>,
    rate_limit: Option<(i32, i32, i32)>,
    http: Option<reqwest::blocking::Client>,
}

impl State {
    fn put(&mut self, r: Res) -> i32 {
        self.next_id += 1;
        let id = self.next_id;
        self.table.insert(id, r);
        id
    }
    fn log(&mut self, what: impl Into<String>) {
        self.calls.push(what.into());
    }
}

fn mem(caller: &mut Caller<'_, State>) -> Result<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Ok(m),
        _ => bail!("guest exports no linear memory"),
    }
}

fn read_str(caller: &mut Caller<'_, State>, ptr: i32, len: i32) -> Result<String> {
    let m = mem(caller)?;
    let mut buf = vec![0u8; len.max(0) as usize];
    m.read(&mut *caller, ptr as usize, &mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--selftest") {
        return selftest();
    }
    let mut args = std::env::args().skip(1);
    let aix = args.next().context("usage: spike <path.aix> [query]")?;
    let query = args.next().unwrap_or_else(|| "one".to_string());

    // ---- unpack the package -------------------------------------------------
    let file = std::fs::File::open(&aix).with_context(|| format!("opening {aix}"))?;
    let mut zip = zip::ZipArchive::new(file)?;

    println!("== package ==");
    let mut names = Vec::new();
    for i in 0..zip.len() {
        let f = zip.by_index(i)?;
        println!("  {:<34} {:>8} bytes", f.name(), f.size());
        names.push(f.name().to_string());
    }

    let manifest: serde_json::Value = {
        let f = zip.by_name("Payload/source.json")?;
        serde_json::from_reader(f)?
    };
    println!("\n== manifest ==\n  {}", manifest);

    let wasm = {
        let mut f = zip.by_name("Payload/main.wasm")?;
        let mut v = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut v)?;
        v
    };

    // ---- engine -------------------------------------------------------------
    // ADR-0004: epoch interruption rather than fuel, and a memory cap.
    let mut config = wasmtime::Config::new();
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(&engine, &wasm)?;

    println!("\n== imports the guest requires ==");
    let mut required: Vec<(String, String)> = Vec::new();
    for imp in module.imports() {
        println!("  {:<10} {:<22} {:?}", imp.module(), imp.name(), imp.ty());
        required.push((imp.module().to_string(), imp.name().to_string()));
    }

    println!("\n== exports the guest provides ==");
    for exp in module.exports() {
        println!("  {:<26} {:?}", exp.name(), exp.ty());
    }

    let mut store = Store::new(
        &engine,
        State {
            http: Some(
                reqwest::blocking::Client::builder()
                    .user_agent("nyuka-spike/0.1")
                    .timeout(std::time::Duration::from_secs(30))
                    .build()?,
            ),
            ..Default::default()
        },
    );
    store.set_epoch_deadline(1);

    let mut linker: Linker<State> = Linker::new(&engine);
    register_host_imports(&mut linker)?;

    // Anything still unresolved becomes a stub that records itself, so one run
    // reports the whole gap instead of failing at the first hole.
    for (m, n) in &required {
        if linker.get(&mut store, m, n).is_err() {
            let (mm, nn) = (m.clone(), n.clone());
            let ty = module
                .imports()
                .find(|i| i.module() == m && i.name() == n)
                .map(|i| i.ty())
                .context("import vanished")?;
            if let Some(ft) = ty.func() {
                let results = ft.results().len();
                let ret: Vec<ValType> = ft.results().collect();
                linker.func_new(
                    &mm.clone(),
                    &nn.clone(),
                    ft.clone(),
                    move |mut c, _a, out| {
                        c.data_mut().missing.push(format!("{mm}::{nn}"));
                        for (i, t) in ret.iter().enumerate().take(results) {
                            out[i] = match t {
                                ValType::I32 => Val::I32(ERR_UNIMPLEMENTED),
                                ValType::I64 => Val::I64(ERR_UNIMPLEMENTED as i64),
                                ValType::F32 => Val::F32(0),
                                ValType::F64 => Val::F64(0),
                                _ => Val::I32(ERR_UNIMPLEMENTED),
                            };
                        }
                        Ok(())
                    },
                )?;
            }
        }
    }

    let instance = linker.instantiate(&mut store, &module)?;
    println!("\n== instantiated ==");

    if let Some(start) = instance.get_func(&mut store, "start") {
        let n = start.ty(&store).results().len();
        let mut out = vec![Val::I32(0); n];
        match start.call(&mut store, &[], &mut out) {
            Ok(()) => println!("  start() -> {out:?}"),
            Err(e) => println!("  start() trapped: {e}"),
        }
    }

    // ---- the entry point we care about -------------------------------------
    //
    // ABI, from the `register_source!` macro in aidoku-rs:
    //
    //   get_search_manga_list(query_descriptor, page, filters_descriptor) -> i32
    //     query   = read_string(d)                  -- raw UTF-8 bytes
    //     filters = read::<Vec<FilterValue>>(d)     -- POSTCARD encoded
    //     returns -1 when the filters fail to deserialize
    //
    // The success return is a POINTER into guest memory, not a handle:
    //   [0..4]  total length including this header (i32 LE)
    //   [4..8]  capacity (i32 LE)
    //   [8..]   postcard payload
    // and it is released with free_result(ptr).
    let name = "get_search_manga_list";
    if let Some(f) = instance.get_func(&mut store, name) {
        let ty = f.ty(&store);
        println!("\n== calling {name} ==");
        println!(
            "  signature: {:?} -> {:?}",
            ty.params().collect::<Vec<_>>(),
            ty.results().collect::<Vec<_>>()
        );

        let query_rid = store
            .data_mut()
            .put(Res::Buffer(query.clone().into_bytes()));
        // postcard for an empty Vec is a single zero-length varint.
        let filters_rid = store.data_mut().put(Res::Buffer(vec![0u8]));
        println!("  query={query:?} (rid {query_rid})  page=1  filters=[] (rid {filters_rid})");

        let args = [Val::I32(query_rid), Val::I32(1), Val::I32(filters_rid)];
        let mut out = [Val::I32(0)];
        match f.call(&mut store, &args, &mut out) {
            Ok(()) => {
                let ret = out[0].unwrap_i32();
                if ret < 0 {
                    println!("  returned error code {ret}");
                } else {
                    match read_result(&mut store, &instance, ret) {
                        Ok((len, cap, payload)) => {
                            println!(
                                "  result ptr={ret} len={len} cap={cap} payload={} bytes",
                                payload.len()
                            );
                            let head: Vec<String> = payload
                                .iter()
                                .take(24)
                                .map(|b| format!("{b:02x}"))
                                .collect();
                            println!("  payload head: {}", head.join(" "));
                            // Printable runs, which is enough to read a
                            // conformance report's (check, result) pairs
                            // without mirroring the guest's structs here.
                            let strings = printable_runs(&payload, 3);
                            if !strings.is_empty() {
                                println!("  strings in payload ({}):", strings.len());
                                for s in &strings {
                                    println!("    {s}");
                                }
                            }
                            // postcard encodes a Vec as a length varint first,
                            // so the leading byte is the entry count for small
                            // results. Enough to show the payload is coherent.
                            if let Some(&n) = payload.first() {
                                println!("  leading varint (likely entry count): {n}");
                            }
                            if let Ok(free) =
                                instance.get_typed_func::<i32, ()>(&mut store, "free_result")
                            {
                                free.call(&mut store, ret)?;
                                println!("  free_result(ptr) ok");
                            }
                        }
                        Err(e) => println!("  could not read result: {e}"),
                    }
                }
            }
            Err(e) => println!("  trapped: {e}"),
        }
    } else {
        println!("\n  no {name} export");
    }

    // ---- what the spike learned -------------------------------------------
    let st = store.data();
    println!("\n== host calls observed ({}) ==", st.calls.len());
    let mut tally: HashMap<&str, usize> = HashMap::new();
    for c in &st.calls {
        *tally.entry(c.as_str()).or_default() += 1;
    }
    let mut rows: Vec<_> = tally.into_iter().collect();
    rows.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (k, v) in rows {
        println!("  {v:>4}x {k}");
    }
    if let Some(rl) = st.rate_limit {
        println!("\n  source declared a rate limit: {rl:?}");
    }
    if st.missing.is_empty() {
        println!("\n  no unimplemented imports were reached");
    } else {
        let mut m: Vec<_> = st.missing.clone();
        m.sort();
        m.dedup();
        println!("\n== unimplemented imports REACHED ==");
        for x in m {
            println!("  {x}");
        }
    }
    println!("\n  live resources at exit: {}", st.table.len());
    Ok(())
}

/// Parses stored HTML and returns the element the handle refers to.
fn as_element(html: &str) -> Option<scraper::Html> {
    Some(scraper::Html::parse_fragment(html))
}

/// The first real element inside a parsed fragment.
fn fragment_root(doc: &scraper::Html) -> Option<scraper::ElementRef<'_>> {
    doc.root_element()
        .children()
        .filter_map(scraper::ElementRef::wrap)
        .next()
        .or(Some(doc.root_element()))
}

fn register_host_imports(linker: &mut Linker<State>) -> Result<()> {
    // ---- env ---------------------------------------------------------------
    // These land in `env` because the corresponding extern block in aidoku-rs
    // declares no explicit wasm import module.
    linker.func_wrap(
        "env",
        "print",
        |mut c: Caller<'_, State>, p: i32, l: i32| {
            let s = read_str(&mut c, p, l).unwrap_or_default();
            c.data_mut().log("env::print");
            println!("    [guest] {s}");
        },
    )?;
    linker.func_wrap("env", "abort", |mut c: Caller<'_, State>| {
        c.data_mut().log("env::abort");
    })?;
    linker.func_wrap(
        "env",
        "send_partial_result",
        |mut c: Caller<'_, State>, _v: i32| {
            // Maps onto job.progress SSE events in the real runtime (ADR-0010).
            c.data_mut().log("env::send_partial_result");
        },
    )?;

    // ---- std ---------------------------------------------------------------
    linker.func_wrap("std", "destroy", |mut c: Caller<'_, State>, rid: i32| {
        c.data_mut().log("std::destroy");
        c.data_mut().table.remove(&rid);
    })?;
    linker.func_wrap(
        "std",
        "buffer_len",
        |mut c: Caller<'_, State>, rid: i32| -> i32 {
            c.data_mut().log("std::buffer_len");
            match c.data().table.get(&rid) {
                Some(Res::Buffer(b)) => b.len() as i32,
                _ => ERR_MESSAGE,
            }
        },
    )?;
    linker.func_wrap(
        "std",
        "read_buffer",
        |mut c: Caller<'_, State>, rid: i32, ptr: i32, len: i32| -> i32 {
            c.data_mut().log("std::read_buffer");
            let bytes = match c.data().table.get(&rid) {
                Some(Res::Buffer(b)) => b.clone(),
                _ => return ERR_MESSAGE,
            };
            let n = bytes.len().min(len.max(0) as usize);
            let Ok(m) = mem(&mut c) else {
                return ERR_MESSAGE;
            };
            match m.write(&mut c, ptr as usize, &bytes[..n]) {
                // 0 means success. The guest's read_buffer does
                // `if error != 0 { return None }`, so returning the byte count
                // makes every successful read look like a failure — which is
                // what produced the -1 from the filters decode.
                Ok(()) => 0,
                Err(_) => ERR_MESSAGE,
            }
        },
    )?;
    linker.func_wrap("std", "current_date", |mut c: Caller<'_, State>| -> f64 {
        c.data_mut().log("std::current_date");
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    })?;
    linker.func_wrap(
        "std",
        "parse_date",
        |mut c: Caller<'_, State>,
         _a: i32,
         _b: i32,
         _d: i32,
         _e: i32,
         _f: i32,
         _g: i32,
         _h: i32,
         _i: i32|
         -> f64 {
            // Real implementation needs the source's format/locale/timezone
            // arguments. Returning 0 keeps the listing path alive.
            c.data_mut().log("std::parse_date (stub)");
            0.0
        },
    )?;
    linker.func_wrap(
        "std",
        "print",
        |mut c: Caller<'_, State>, p: i32, l: i32| {
            let s = read_str(&mut c, p, l).unwrap_or_default();
            c.data_mut().log("std::print");
            println!("    [guest] {s}");
        },
    )?;
    linker.func_wrap("std", "abort", |mut c: Caller<'_, State>| {
        c.data_mut().log("std::abort");
    })?;

    // ---- net ---------------------------------------------------------------
    linker.func_wrap(
        "net",
        "init",
        |mut c: Caller<'_, State>, method: i32| -> i32 {
            c.data_mut().log("net::init");
            c.data_mut().put(Res::Request(Req {
                method,
                ..Default::default()
            }))
        },
    )?;
    linker.func_wrap(
        "net",
        "set_url",
        |mut c: Caller<'_, State>, rid: i32, p: i32, l: i32| -> i32 {
            let url = match read_str(&mut c, p, l) {
                Ok(s) => s,
                Err(_) => return ERR_REQUEST,
            };
            c.data_mut().log("net::set_url");
            match c.data_mut().table.get_mut(&rid) {
                Some(Res::Request(r)) => {
                    r.url = Some(url);
                    0
                }
                _ => ERR_REQUEST,
            }
        },
    )?;
    linker.func_wrap(
        "net",
        "set_header",
        |mut c: Caller<'_, State>, rid: i32, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
            let k = read_str(&mut c, kp, kl).unwrap_or_default();
            let v = read_str(&mut c, vp, vl).unwrap_or_default();
            c.data_mut().log("net::set_header");
            match c.data_mut().table.get_mut(&rid) {
                Some(Res::Request(r)) => {
                    r.headers.push((k, v));
                    0
                }
                _ => ERR_REQUEST,
            }
        },
    )?;
    linker.func_wrap(
        "net",
        "set_body",
        |mut c: Caller<'_, State>, rid: i32, p: i32, l: i32| -> i32 {
            let body = read_str(&mut c, p, l).unwrap_or_default().into_bytes();
            c.data_mut().log("net::set_body");
            match c.data_mut().table.get_mut(&rid) {
                Some(Res::Request(r)) => {
                    r.body = Some(body);
                    0
                }
                _ => ERR_REQUEST,
            }
        },
    )?;
    linker.func_wrap(
        "net",
        "set_rate_limit",
        |mut c: Caller<'_, State>, p: i32, per: i32, u: i32| {
            // ADR-0004: the source declares its own limit; the job engine takes the
            // stricter of this and the configured cap.
            c.data_mut().log("net::set_rate_limit");
            c.data_mut().rate_limit = Some((p, per, u));
        },
    )?;
    linker.func_wrap("net", "send", |mut c: Caller<'_, State>, rid: i32| -> i32 {
        c.data_mut().log("net::send");
        let (method, url, headers, body) = match c.data().table.get(&rid) {
            Some(Res::Request(r)) => (r.method, r.url.clone(), r.headers.clone(), r.body.clone()),
            _ => return ERR_REQUEST,
        };
        let Some(url) = url else { return ERR_REQUEST };
        // Verb ordering follows aidoku-rs `HttpMethod`. Guessed, and worth
        // confirming against the guest before this leaves the spike.
        let verb = match method {
            1 => reqwest::Method::POST,
            2 => reqwest::Method::PUT,
            3 => reqwest::Method::HEAD,
            4 => reqwest::Method::DELETE,
            _ => reqwest::Method::GET,
        };
        println!("    [net] {verb} {url}");
        let client = match c.data().http.clone() {
            Some(cl) => cl,
            None => return ERR_REQUEST,
        };
        let mut req = client.request(verb, &url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        match req.send().and_then(|r| r.bytes()) {
            Ok(bytes) => {
                println!("    [net] {} bytes", bytes.len());
                if let Some(Res::Request(r)) = c.data_mut().table.get_mut(&rid) {
                    r.response = Some(bytes.to_vec());
                }
                0
            }
            Err(e) => {
                println!("    [net] error: {e}");
                ERR_REQUEST
            }
        }
    })?;
    linker.func_wrap(
        "net",
        "data_len",
        |mut c: Caller<'_, State>, rid: i32| -> i32 {
            c.data_mut().log("net::data_len");
            match c.data().table.get(&rid) {
                Some(Res::Request(r)) => {
                    r.response.as_ref().map_or(ERR_REQUEST, |b| b.len() as i32)
                }
                _ => ERR_REQUEST,
            }
        },
    )?;
    linker.func_wrap(
        "net",
        "read_data",
        |mut c: Caller<'_, State>, rid: i32, ptr: i32, len: i32| -> i32 {
            c.data_mut().log("net::read_data");
            let bytes = match c.data().table.get(&rid) {
                Some(Res::Request(r)) => match &r.response {
                    Some(b) => b.clone(),
                    None => return ERR_REQUEST,
                },
                _ => return ERR_REQUEST,
            };
            let n = bytes.len().min(len.max(0) as usize);
            let Ok(m) = mem(&mut c) else {
                return ERR_REQUEST;
            };
            match m.write(&mut c, ptr as usize, &bytes[..n]) {
                Ok(()) => 0, // success is 0, as with std::read_buffer
                Err(_) => ERR_REQUEST,
            }
        },
    )?;
    linker.func_wrap("net", "html", |mut c: Caller<'_, State>, rid: i32| -> i32 {
        c.data_mut().log("net::html");
        let (body, base) = match c.data().table.get(&rid) {
            Some(Res::Request(r)) => match &r.response {
                Some(b) => (
                    String::from_utf8_lossy(b).into_owned(),
                    r.url.clone().map(Rc::new),
                ),
                None => return ERR_REQUEST,
            },
            _ => return ERR_REQUEST,
        };
        c.data_mut().put(Res::Node {
            html: Rc::new(body),
            base,
        })
    })?;

    // ---- html --------------------------------------------------------------
    linker.func_wrap(
        "html",
        "parse_fragment",
        |mut c: Caller<'_, State>, p: i32, l: i32, bp: i32, bl: i32| -> i32 {
            let s = read_str(&mut c, p, l).unwrap_or_default();
            let base = read_str(&mut c, bp, bl).unwrap_or_default();
            c.data_mut().log("html::parse_fragment");
            let base = if base.is_empty() {
                None
            } else {
                Some(Rc::new(base))
            };
            c.data_mut().put(Res::Node {
                html: Rc::new(s),
                base,
            })
        },
    )?;
    linker.func_wrap(
        "html",
        "select",
        |mut c: Caller<'_, State>, rid: i32, p: i32, l: i32| -> i32 {
            let sel = read_str(&mut c, p, l).unwrap_or_default();
            c.data_mut().log("html::select");
            let (html, base) = match c.data().table.get(&rid) {
                Some(Res::Node { html, base }) => (html.clone(), base.clone()),
                _ => return HTML_INVALID_DESCRIPTOR,
            };
            let Ok(parsed) = scraper::Selector::parse(&sel) else {
                return HTML_INVALID_QUERY;
            };
            let Some(doc) = as_element(&html) else {
                return HTML_INVALID_DESCRIPTOR;
            };
            let root = fragment_root(&doc).map(|e| e.id());
            let hits: Vec<String> = doc
                .select(&parsed)
                .filter(|e| Some(e.id()) != root)
                .map(|e| e.html())
                .collect();
            c.data_mut().put(Res::NodeList {
                items: Rc::new(hits),
                base,
            })
        },
    )?;
    linker.func_wrap(
        "html",
        "select_first",
        |mut c: Caller<'_, State>, rid: i32, p: i32, l: i32| -> i32 {
            let sel = read_str(&mut c, p, l).unwrap_or_default();
            c.data_mut().log("html::select_first");
            let (html, base) = match c.data().table.get(&rid) {
                Some(Res::Node { html, base }) => (html.clone(), base.clone()),
                _ => return HTML_INVALID_DESCRIPTOR,
            };
            let Ok(parsed) = scraper::Selector::parse(&sel) else {
                return HTML_INVALID_QUERY;
            };
            let Some(doc) = as_element(&html) else {
                return HTML_INVALID_DESCRIPTOR;
            };
            let root = fragment_root(&doc).map(|e| e.id());
            match doc.select(&parsed).find(|e| Some(e.id()) != root) {
                Some(e) => {
                    let h = e.html();
                    c.data_mut().put(Res::Node {
                        html: Rc::new(h),
                        base,
                    })
                }
                // A legitimate miss, not a bad query.
                None => HTML_NO_RESULT,
            }
        },
    )?;
    linker.func_wrap(
        "html",
        "size",
        |mut c: Caller<'_, State>, rid: i32| -> i32 {
            c.data_mut().log("html::size");
            match c.data().table.get(&rid) {
                Some(Res::NodeList { items, .. }) => items.len() as i32,
                Some(Res::Node { .. }) => 1,
                _ => HTML_INVALID_DESCRIPTOR,
            }
        },
    )?;
    linker.func_wrap(
        "html",
        "get",
        |mut c: Caller<'_, State>, rid: i32, idx: i32| -> i32 {
            c.data_mut().log("html::get");
            let (item, base) = match c.data().table.get(&rid) {
                Some(Res::NodeList { items, base }) => {
                    (items.get(idx.max(0) as usize).cloned(), base.clone())
                }
                _ => (None, None),
            };
            match item {
                Some(h) => c.data_mut().put(Res::Node {
                    html: Rc::new(h),
                    base,
                }),
                None => HTML_NO_RESULT,
            }
        },
    )?;
    linker.func_wrap(
        "html",
        "attr",
        |mut c: Caller<'_, State>, rid: i32, p: i32, l: i32| -> i32 {
            let key = read_str(&mut c, p, l).unwrap_or_default();
            c.data_mut().log("html::attr");
            let (html, base) = match c.data().table.get(&rid) {
                Some(Res::Node { html, base }) => (html.clone(), base.clone()),
                _ => return HTML_INVALID_DESCRIPTOR,
            };
            // Jsoup's convention: an `abs:` prefix means resolve the value
            // against the document base. A host that treats `abs:href` as a
            // literal attribute name finds nothing, and the source silently
            // discards the entry — which is what made every HTML-scraping
            // source return zero results.
            let (want_abs, key) = match key.strip_prefix("abs:") {
                Some(rest) => (true, rest.to_string()),
                None => (false, key),
            };
            let Some(doc) = as_element(&html) else {
                return HTML_INVALID_DESCRIPTOR;
            };
            let Some(raw) =
                fragment_root(&doc).and_then(|e| e.value().attr(&key).map(str::to_owned))
            else {
                return HTML_NO_RESULT;
            };
            let val = if want_abs {
                match resolve(base.as_deref(), &raw) {
                    Some(abs) => abs,
                    None => return HTML_NO_RESULT,
                }
            } else {
                raw
            };
            c.data_mut().put(Res::Buffer(val.into_bytes()))
        },
    )?;
    for (name, own) in [("text", false), ("own_text", true)] {
        linker.func_wrap(
            "html",
            name,
            move |mut c: Caller<'_, State>, rid: i32| -> i32 {
                c.data_mut().log(format!("html::{name}"));
                let html = match c.data().table.get(&rid) {
                    Some(Res::Node { html, .. }) => html.clone(),
                    _ => return HTML_INVALID_DESCRIPTOR,
                };
                let Some(doc) = as_element(&html) else {
                    return HTML_INVALID_DESCRIPTOR;
                };
                let s = match fragment_root(&doc) {
                    Some(e) if own => e
                        .children()
                        .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
                        .collect::<String>(),
                    Some(e) => e.text().collect::<String>(),
                    None => String::new(),
                };
                let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
                c.data_mut().put(Res::Buffer(s.into_bytes()))
            },
        )?;
    }
    linker.func_wrap(
        "html",
        "html",
        |mut c: Caller<'_, State>, rid: i32| -> i32 {
            c.data_mut().log("html::html");
            let html = match c.data().table.get(&rid) {
                Some(Res::Node { html, .. }) => html.to_string(),
                _ => return HTML_INVALID_DESCRIPTOR,
            };
            c.data_mut().put(Res::Buffer(html.into_bytes()))
        },
    )?;
    linker.func_wrap(
        "html",
        "base_uri",
        |mut c: Caller<'_, State>, rid: i32| -> i32 {
            c.data_mut().log("html::base_uri");
            match c.data().table.get(&rid) {
                Some(Res::Node { base: Some(b), .. }) => {
                    let bytes = b.as_bytes().to_vec();
                    c.data_mut().put(Res::Buffer(bytes))
                }
                Some(Res::Node { base: None, .. }) => HTML_NO_RESULT,
                _ => HTML_INVALID_DESCRIPTOR,
            }
        },
    )?;
    linker.func_wrap(
        "html",
        "set_text",
        |mut c: Caller<'_, State>, _rid: i32, _p: i32, _l: i32| -> i32 {
            // `scraper` cannot mutate. Whether this is reached on the listing
            // path is one of the questions the spike answers (ADR-0004).
            c.data_mut()
                .log("html::set_text (UNIMPLEMENTED — needs a mutable DOM)");
            ERR_UNIMPLEMENTED
        },
    )?;

    // ---- defaults ----------------------------------------------------------
    linker.func_wrap(
        "defaults",
        "get",
        |mut c: Caller<'_, State>, p: i32, l: i32| -> i32 {
            let key = read_str(&mut c, p, l).unwrap_or_default();
            c.data_mut().log(format!("defaults::get({key})"));
            ERR_MESSAGE // nothing stored: the real adapter reads source_kv
        },
    )?;
    linker.func_wrap(
        "defaults",
        "set",
        |mut c: Caller<'_, State>, _p: i32, _l: i32, _k: i32, _v: i32| -> i32 {
            c.data_mut().log("defaults::set");
            0
        },
    )?;

    Ok(())
}

/// Reads the `[len, capacity, payload…]` block the guest returns.
fn read_result(
    store: &mut Store<State>,
    instance: &wasmtime::Instance,
    ptr: i32,
) -> Result<(i32, i32, Vec<u8>)> {
    let Some(Extern::Memory(m)) = instance.get_export(&mut *store, "memory") else {
        bail!("guest exports no memory")
    };
    let mut header = [0u8; 8];
    m.read(&mut *store, ptr as usize, &mut header)?;
    let len = i32::from_le_bytes(header[0..4].try_into()?);
    let cap = i32::from_le_bytes(header[4..8].try_into()?);
    if len < 8 {
        bail!("implausible result length {len}");
    }
    let mut payload = vec![0u8; (len - 8) as usize];
    m.read(&mut *store, (ptr + 8) as usize, &mut payload)?;
    Ok((len, cap, payload))
}

/// Exercises the spike's node model directly, so a wrong result can be
/// attributed to the HTML layer rather than to the ABI.
fn selftest() -> Result<()> {
    let page = r#"<html><body>
      <div class="grid">
        <a class="card" href="/series/1"><img src="/c1.jpg" alt="Alpha"><h3>Alpha</h3></a>
        <a class="card" href="/series/2"><img src="/c2.jpg" alt="Beta"><h3>Beta</h3></a>
      </div></body></html>"#;

    println!("== node model selftest ==");
    let doc = as_element(page).context("parse failed")?;
    let sel = scraper::Selector::parse("a.card").unwrap();
    let hits: Vec<String> = doc.select(&sel).map(|e| e.html()).collect();
    println!("  select('a.card') -> {} hits", hits.len());

    for (i, outer) in hits.iter().enumerate() {
        // This is exactly what html::get followed by attr/select_first does:
        // re-parse the stored outer HTML, then query inside it.
        let sub = as_element(outer).context("re-parse failed")?;
        let href = fragment_root(&sub)
            .and_then(|e| e.value().attr("href"))
            .unwrap_or("<none>");
        let h3 = scraper::Selector::parse("h3").unwrap();
        let title = sub.select(&h3).next().map(|e| e.text().collect::<String>());
        let img = scraper::Selector::parse("img").unwrap();
        let cover = sub
            .select(&img)
            .next()
            .and_then(|e| e.value().attr("src").map(str::to_owned));
        println!("  [{i}] href={href:?} title={title:?} cover={cover:?}");
    }
    Ok(())
}

/// Printable ASCII runs of at least `min` characters, in order.
fn printable_runs(bytes: &[u8], min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for &b in bytes {
        if (0x20..0x7f).contains(&b) {
            cur.push(b as char);
        } else {
            if cur.chars().count() >= min {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    if cur.chars().count() >= min {
        out.push(cur);
    }
    out
}

/// Resolves a possibly-relative URL against a base, for the `abs:` prefix.
fn resolve(base: Option<&String>, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    // Already absolute.
    if let Ok(u) = url::Url::parse(value)
        && (u.scheme() == "http" || u.scheme() == "https")
    {
        return Some(u.to_string());
    }
    let base = url::Url::parse(base?).ok()?;
    base.join(value).ok().map(|u| u.to_string())
}
