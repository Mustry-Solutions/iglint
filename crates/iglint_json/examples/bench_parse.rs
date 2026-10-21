//! Parse throughput on a generated 5 MB view-like document.
//!
//! `cargo run -p iglint_json --release --example bench_parse`

use std::fmt::Write;
use std::time::Instant;

fn generate(target: usize) -> String {
    let mut out =
        String::from("{\n  \"custom\": {},\n  \"params\": {},\n  \"root\": {\n    \"children\": [");
    let mut i = 0usize;
    while out.len() < target {
        if i > 0 {
            out.push(',');
        }
        write!(
            out,
            "\n      {{\n        \"events\": {{\n          \"dom\": {{\n            \"onClick\": {{\n              \"config\": {{\n                \"script\": \"\\tvalue \\u003d self.view.custom.counter\\n\\tself.view.custom.counter \\u003d value + {i}\"\n              }},\n              \"scope\": \"G\",\n              \"type\": \"script\"\n            }}\n          }}\n        }},\n        \"meta\": {{\n          \"name\": \"Button_{i}\"\n        }},\n        \"position\": {{\n          \"basis\": \"{basis}px\",\n          \"grow\": 1,\n          \"shrink\": 0.5\n        }},\n        \"propConfig\": {{\n          \"props.text\": {{\n            \"binding\": {{\n              \"config\": {{\n                \"expression\": \"if({{view.params.mode}} \\u003d \\u0027edit\\u0027, \\\"Save\\\", \\\"Edit\\\")\"\n              }},\n              \"type\": \"expr\"\n            }}\n          }}\n        }},\n        \"props\": {{\n          \"enabled\": true,\n          \"style\": {{\n            \"classes\": \"buttons/primary\",\n            \"marginTop\": \"4px\"\n          }},\n          \"text\": \"Label {i} é€\"\n        }},\n        \"type\": \"ia.input.button\"\n      }}",
            basis = i % 400
        )
        .unwrap();
        i += 1;
    }
    out.push_str("\n    ],\n    \"meta\": {\n      \"name\": \"root\"\n    },\n    \"type\": \"ia.container.flex\"\n  }\n}");
    out
}

fn main() {
    let text = generate(5 * 1024 * 1024);
    let root = iglint_json::parse(&text).expect("generated document parses");
    assert!(root.get("root").is_some());
    drop(root);
    let mb = text.len() as f64 / (1024.0 * 1024.0);
    let mut best = f64::MAX;
    let rounds = 30;
    let start_all = Instant::now();
    for _ in 0..rounds {
        let start = Instant::now();
        let v = iglint_json::parse(&text).unwrap();
        std::hint::black_box(&v);
        best = best.min(start.elapsed().as_secs_f64());
    }
    let mean = start_all.elapsed().as_secs_f64() / rounds as f64;
    println!(
        "{:.2} MiB: best {:.2} ms ({:.0} MiB/s), mean {:.2} ms ({:.0} MiB/s)",
        mb,
        best * 1000.0,
        mb / best,
        mean * 1000.0,
        mb / mean
    );
}
