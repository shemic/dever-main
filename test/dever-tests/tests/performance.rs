mod support;

use std::path::Path;
use std::process::Command;
use std::time::Instant;
use support::{checked, sources, temp::TemporaryDirectory};

struct Kernel {
    name: &'static str,
    bytes: usize,
    dever: &'static str,
    rust: &'static str,
    decimal_initial: bool,
    expected: String,
}

const SAMPLES: usize = 9;
const PROCESSES: usize = 25;
const MAX_RATIO: f64 = 1.15;

#[test]
#[ignore = "explicit bounded record/list/JSON/large-map benchmark"]
fn allocation_sensitive_source_scenarios() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/performance");
    let sources = dever_core::source::SourceMap::load(&root).unwrap();
    let program = checked(&sources);
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let executable =
        dever_core::native::compile(&program, &sources, "main.main", &compiler).unwrap();
    for process in 0..3 {
        let output = Command::new(executable.executable()).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let mut counts = std::collections::BTreeMap::<&str, usize>::new();
        for line in stdout.lines() {
            let fields: Vec<_> = line.split('|').collect();
            assert_eq!(fields.len(), 3, "{line}");
            let expected = match fields[0] {
                "text" | "list" | "map" => "16384",
                "json" => "1",
                _ => panic!("unexpected result: {line}"),
            };
            assert_eq!(fields[1], expected);
            assert!(fields[2].parse::<u64>().unwrap() > 0);
            *counts.entry(fields[0]).or_default() += 1;
            println!("process={process} {line}");
        }
        assert_eq!(
            counts,
            std::collections::BTreeMap::from([("text", 6), ("list", 6), ("json", 6), ("map", 1)])
        );
    }
}

#[test]
#[ignore = "explicit bounded native hot-path benchmark; never part of default checks"]
fn native_hot_paths_stay_within_the_matching_runtime_baseline() {
    let kernels = [
        Kernel {
            name: "checked_int_reduce",
            bytes: 1_048_576,
            dever: "result = int.to_text(reduce(add_int, bytes, 0))",
            rust: "let mut total = 0i64; for value in bytes.values() { total = number::int_add(total, i64::from(*value)).unwrap(); } total.to_string()",
            expected: "133693440".into(),
            decimal_initial: false,
        },
        Kernel {
            name: "float_reduce",
            bytes: 1_048_576,
            dever: "result = float.to_text(reduce(add_float, bytes, float.from_int(0)))",
            rust: "let mut total = 0.0f64; for value in bytes.values() { total += f64::from(*value); } total.to_string()",
            expected: "133693440".into(),
            decimal_initial: false,
        },
        Kernel {
            name: "list_each_filter_sum",
            bytes: 262_144,
            dever: "values = each(identity, bytes)\nselected = filter(even, values)\nmapped = each(triple, selected)\nresult = int.to_text(sum(mapped))",
            rust: "let values = List::new(bytes.values().iter().map(|value| i64::from(*value)).collect()); let selected = List::new(values.values().iter().copied().filter(|value| number::int_rem(*value, 2).unwrap() == 0).collect()); let mapped = List::new(selected.values().iter().map(|value| number::int_mul(*value, 3).unwrap()).collect()); let mut total = 0i64; for value in mapped.values() { total = number::int_add(total, *value).unwrap(); } total.to_string()",
            expected: "49938432".into(),
            decimal_initial: false,
        },
        Kernel {
            name: "decimal_reduce",
            bytes: 16_384,
            dever: "result = decimal.to_text(reduce(add_decimal, bytes, 0.0))",
            rust: "let mut total = decimal_initial; for value in bytes.values() { total = total.checked_add(DecimalValue::from_int(i64::from(*value))).unwrap(); } total.to_string()",
            expected: "2.08896E+6".into(),
            decimal_initial: true,
        },
        Kernel {
            name: "ordered_map_reduce",
            bytes: 65_536,
            dever: "values = reduce(map_put, bytes, {})\nresult = int.to_text(length(values))",
            rust: "let mut values = Map::new(Vec::new()).unwrap(); for value in bytes.values() { let value = i64::from(*value); values = values.put(value, number::int_mul(value, 3).unwrap()); } values.pairs().count().to_string()",
            expected: "256".into(),
            decimal_initial: false,
        },
    ];
    let directory = TemporaryDirectory::new();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut failures = Vec::new();
    for kernel in kernels {
        let input = directory.path().join(format!("{}.bin", kernel.name));
        std::fs::write(
            &input,
            (0..kernel.bytes)
                .map(|index| index as u8)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let source = dever_benchmark(&kernel, &input);
        let sources = sources(&source);
        let program = checked(&sources);
        let native_started = Instant::now();
        let native =
            dever_core::native::compile(&program, &sources, "main.main", &compiler).unwrap();
        let native_build_ms = native_started.elapsed().as_millis();
        let baseline_source = directory.path().join(format!("{}.rs", kernel.name));
        let baseline = directory.path().join(kernel.name);
        std::fs::write(&baseline_source, rust_benchmark(&kernel, &input)).unwrap();
        let runtime = native.executable().parent().unwrap().join("runtime");
        let baseline_started = Instant::now();
        let compiled = Command::new(&compiler)
            .arg("--edition=2024")
            .args(dever_core::native::OPTIMIZATION_ARGS)
            .arg("--extern")
            .arg(format!(
                "dever_runtime={}",
                runtime.join("libdever_runtime.rlib").display()
            ))
            .arg("-L")
            .arg(format!("dependency={}", runtime.join("deps").display()))
            .arg(&baseline_source)
            .arg("-o")
            .arg(&baseline)
            .output()
            .unwrap();
        let baseline_build_ms = baseline_started.elapsed().as_millis();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        // 分别测量进程内部热路径；构建、文件读取和进程启动不计入时间。
        let (native_nanos, baseline_nanos) = medians(
            kernel.name,
            native.executable(),
            &baseline,
            &kernel.expected,
        );
        let ratio = native_nanos as f64 / baseline_nanos as f64;
        println!(
            "{}: native={}ns baseline={}ns ratio={ratio:.3} budget={MAX_RATIO:.2} native_build={native_build_ms}ms baseline_build={baseline_build_ms}ms",
            kernel.name, native_nanos, baseline_nanos
        );
        if ratio > MAX_RATIO {
            failures.push(format!("{}: {ratio:.3}", kernel.name));
        }
    }
    assert!(
        failures.is_empty(),
        "hot-path budget exceeded: {}",
        failures.join(", ")
    );
}

fn medians(kernel: &str, native: &Path, baseline: &Path, expected: &str) -> (u64, u64) {
    let executables = [native, baseline];
    let mut measurements = [Vec::new(), Vec::new()];
    // 同一 ELF 在共享宿主上也会出现不同耗时档；交错足够多的独立进程，
    // 减少五组采样偶然偏向一档的风险，并保留分组证据供后续诊断。
    for process in 0..PROCESSES {
        let order = if process % 2 == 0 { [0, 1] } else { [1, 0] };
        for index in order {
            let samples = measure(executables[index], expected);
            println!(
                "{kernel} process={process} side={} samples={samples:?}",
                ["native", "baseline"][index]
            );
            measurements[index].extend(samples);
        }
    }
    let [native, baseline] = measurements.map(|mut values| {
        values.sort_unstable();
        values[values.len() / 2]
    });
    (native, baseline)
}

fn measure(executable: &Path, expected: &str) -> Vec<u64> {
    let output = Command::new(executable).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut samples = Vec::new();
    for line in stdout.lines() {
        let (result, nanos) = line
            .split_once('|')
            .unwrap_or_else(|| panic!("invalid benchmark output: {line}"));
        assert_eq!(result, expected);
        samples.push(nanos.parse::<u64>().unwrap());
    }
    assert_eq!(samples.len(), SAMPLES + 1);
    samples.remove(0); // 同样舍弃首个预热样本。
    assert!(samples.iter().all(|sample| *sample > 0));
    samples
}

fn dever_benchmark(kernel: &Kernel, input: &Path) -> String {
    format!(
        r#"public main() () {{ opened(result(dever.io.open({input:?}))) }}
opened(result: dever.io.OpenResult.Opened(file)) () recover("benchmark reports IO failures") {{
  read(result(dever.io.read(file, {bytes})))
  dever.io.close(file)
}}
opened(result: dever.io.OpenResult.Failed(message)) () recover("benchmark reports IO failures") {{ dever.io.println("FAILED:" + message) }}
read(result: dever.io.ReadResult.Done(state)) () recover("benchmark reports IO failures") {{ read_state(state) }}
read(result: other) () recover("benchmark reports IO failures") {{ dever.io.println("FAILED:input") }}
read_state(state: dever.io.ReadState.Read(bytes)) () {{ each(sample, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9], bytes) }}
read_state(state: other) () {{ dever.io.println("FAILED:input") }}
clock(result: dever.time.ClockResult.Read(value)) (answer: Int) recover("benchmark reports clock failures") {{ answer = value }}
clock(result: dever.time.ClockResult.Failed(message)) (answer: Int) recover("benchmark reports clock failures") {{ dever.io.println("FAILED:" + message)
answer = -1 }}
sample(index: Int, bytes: Bytes) () {{
  start = clock(result(dever.time.monotonic_nanos()))
  result = work(bytes)
  elapsed = clock(result(dever.time.monotonic_nanos())) - start
  dever.io.println(result + "|" + int.to_text(elapsed))
}}
work(bytes: Bytes) (result: Text) {{ {body} }}
add_int(value: Int, total: Int) (next: Int) {{ next = total + value }}
identity(value: Int) (result: Int) {{ result = value }}
even(value: Int) (result: Bool) {{ result = value % 2 == 0 }}
triple(value: Int) (result: Int) {{ result = value * 3 }}
add_decimal(value: Int, total: Decimal) (next: Decimal) {{ next = total + value }}
add_float(value: Int, total: Float) (next: Float) {{ next = total + float.from_int(value) }}
map_put(value: Int, values: Map<Int, Int>) (next: Map<Int, Int>) {{ next = put(values, value, value * 3) }}
"#,
        input = input.to_str().unwrap(),
        bytes = kernel.bytes,
        body = kernel.dever
    )
}

fn rust_benchmark(kernel: &Kernel, input: &Path) -> String {
    // Decimal 的 quantum 属于表示：0.0 与 ZERO 不可作为同成本输入比较。
    // 只给 Decimal 内核传入一次性解析的初值，解析不进入计时区间。
    let (parameters, setup, arguments) = if kernel.decimal_initial {
        (
            ", decimal_initial: DecimalValue",
            "let decimal_initial = DecimalValue::parse(\"0.0\").unwrap();",
            ", decimal_initial",
        )
    } else {
        ("", "", "")
    };
    format!(
        r#"use dever_runtime::{{bytes::Bytes, collections::{{List, Map}}, number::{{self, DecimalValue}}, time}};
fn work(bytes: &Bytes{parameters}) -> String {{ {body} }}
fn main() {{
    let bytes = Bytes::new(std::fs::read({input:?}).unwrap());
    {setup}
    for _ in 0..10 {{
        let start = time::monotonic_nanos().unwrap();
        let result = work(std::hint::black_box(&bytes){arguments});
        let elapsed = time::monotonic_nanos().unwrap() - start;
        println!("{{result}}|{{elapsed}}");
    }}
}}
"#,
        input = input.to_str().unwrap(),
        body = kernel.rust
    )
}
