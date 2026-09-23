//! Ground truth for the client's postcard codec.
//!
//! A source's settings cross the wire as postcard bytes, and the web client
//! decodes them to render an editor (`web/src/features/sources/lib/postcard.ts`).
//! That codec is hand-written in TypeScript, so it is held to the real
//! serializer by these vectors rather than to a reading of the format.
//!
//! Run with `--nocapture` to print them.
#[test]
fn print_setting_value_vectors() {
    macro_rules! show {
        ($label:expr, $value:expr) => {
            let bytes = postcard::to_allocvec(&$value).expect("encode");
            println!("VECTOR {} = {:?}", $label, bytes);
        };
    }

    show!("bool:true", true);
    show!("bool:false", false);
    show!("str:empty", "");
    show!("str:on", "on");
    show!("str:ongoing", "ongoing");
    // Multi-byte UTF-8, to check the length is counted in bytes not characters.
    show!("str:nihongo", "日本語");
    // Long enough that the length crosses the one-byte varint boundary.
    show!("str:200x", "x".repeat(200));
    show!("vec:empty", Vec::<String>::new());
    show!("vec:a,bb", vec!["a".to_string(), "bb".to_string()]);
    show!("i32:0", 0i32);
    show!("i32:1", 1i32);
    show!("i32:-1", -1i32);
    show!("i32:300", 300i32);
}
