//! `tauron_test_sidecar` —— 被 `tauron_proc::CommandSpawner` **真的**起成操作系统
//! 进程的 stdio JSON-RPC 帧回路端（CI 夹具）。
//!
//! 行为全部由 argv 指挥（见 `tauron_test_sidecar::Spec`）；本文件只负责把
//! [`Behavior`] 的每一种落到最小实现上。协议照 `crates/tauron-proc` +
//! `crates/tauron-adapter` 的**现状**实现：一行一帧、`\n` 分帧、回帧必须带同一个
//! `callId`；**没有握手帧**（`tauron-proc` 里不存在这个东西，不在这里发明）。

use std::collections::VecDeque;
use std::io::BufRead;
use std::time::Duration;

use serde_json::{json, Map, Value};
use tauron_test_sidecar::{Behavior, SidecarStdout, Spec, Trace, FLUSH_TRIGGER};

fn main() -> std::process::ExitCode {
    let spec = match Spec::from_args(std::env::args_os().skip(1)) {
        Ok(spec) => spec,
        Err(err) => {
            eprintln!("tauron-test-sidecar: {err}");
            return std::process::ExitCode::from(2);
        }
    };
    let mut trace = Trace::new(spec.trace.clone());
    // SAFETY: 进程刚起来，继承来的 stdout 句柄还没有第二个所有者（本夹具从此不碰
    // `std::io::stdout()`，日志走 stderr）。
    let mut out = unsafe { SidecarStdout::inherit() };
    let code = run(&spec, &mut trace, &mut out);
    trace.record("exit", map([("exitCode", json!(code))]));
    std::process::exit(code);
}

fn map<const N: usize>(items: [(&str, Value); N]) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in items {
        out.insert(key.to_string(), value);
    }
    out
}

/// 主循环。返回进程退出码。
///
/// 这里**不**用 `std::process::exit` 表达"崩溃"：崩溃就是返回非零码，
/// 由 `main` 统一出口，保证 `--trace` 的最后一行一定写得出。
fn run(spec: &Spec, trace: &mut Trace, out: &mut SidecarStdout) -> i32 {
    trace.record(
        "start",
        map([
            ("behavior", json!(spec.behavior.as_str())),
            ("sidecarPid", json!(std::process::id())),
            ("replyOnTrigger", json!(spec.reply_on_trigger)),
            ("fanout", json!(spec.fanout)),
        ]),
    );

    // `crash` + `--exit-after-requests 0` = 一起来就崩（一条帧都不收）。
    if matches!(spec.behavior, Behavior::Crash) && spec.exit_after_requests == 0 {
        trace.record("crash-before-first-frame", map([]));
        return spec.exit_code;
    }

    // `deaf`：**一眼都不看 stdin**，只是活着（轮 2 写侧背压的驱动源）。
    // 先落一笔自证再停住——测试据此区分「真起了一个装聋的进程」与「进程没起来」，
    // 后者会让背压断言空跑。之后就等宿主 `kill`（stdin 被关也不改变行为）。
    if matches!(spec.behavior, Behavior::Deaf) {
        trace.record("deaf-parked", map([("sidecarPid", json!(std::process::id()))]));
        loop {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut served: u64 = 0;
    // 攒着还没吐出去的回帧（`--reply-on-trigger` 与 `--out-of-order` 共用）。
    let mut staged: VecDeque<Vec<u8>> = VecDeque::new();

    loop {
        let line = match lines.next() {
            None => break, // 宿主关了 stdin（kill 之后的正常收尾）
            Some(Err(_)) => break,
            Some(Ok(line)) => line,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(trimmed) else {
            // 污染行：如实忽略，绝不回帧（协议里没有"拒绝帧"这个东西）。
            trace.record("unparsed-line", map([]));
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        if method == FLUSH_TRIGGER {
            // 控制帧：只负责把攒下的回帧吐出去，自己**不**产生回帧。
            served = served.saturating_add(1);
            let flushed = flush(trace, out, &mut staged);
            trace.record("flushed", map([("frames", json!(flushed))]));
            if !flushed {
                return spec.exit_code;
            }
            continue;
        }
        served = served.saturating_add(1);
        trace.record(
            "request-received",
            map([
                ("method", json!(method)),
                ("callId", request.get("callId").cloned().unwrap_or(Value::Null)),
                ("requestId", request.get("id").cloned().unwrap_or(Value::Null)),
                (
                    "runtimeGeneration",
                    request.get("runtimeGeneration").cloned().unwrap_or(Value::Null),
                ),
                ("served", json!(served)),
            ]),
        );

        if matches!(spec.behavior, Behavior::Crash) && served >= spec.exit_after_requests {
            trace.record("crash-after-request", map([("served", json!(served))]));
            return spec.exit_code;
        }
        if spec.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(spec.delay_ms));
        }

        match spec.behavior {
            // 收了不回：超时路径与写侧背压路径的驱动源。
            Behavior::Silent => continue,
            Behavior::Oversize => {
                let frame = oversized_frame(&request, spec.frame_bytes);
                trace.record("oversize-written", map([("bytes", json!(frame.len() + 1))]));
                if !out.write_line(&frame) {
                    trace.record("stdout-broken-pipe", map([]));
                    return spec.exit_code;
                }
                // 吐完这条帧**不退**：读干 stdin 再退。于是宿主侧「断开帧回路」与
                // 「终止进程」是两个能分别观察的事实——否则超帧之后紧跟 EOF，
                // 测试就没法证明"只断帧、没杀进程"这一步。
                while lines.next().is_some() {}
                return spec.exit_code;
            }
            _ => {}
        }

        for _ in 0..spec.fanout {
            let frame = reply_frame(spec, &request);
            if spec.out_of_order > 0 || spec.reply_on_trigger {
                staged.push_back(frame);
            } else if !out.write_line(&frame) {
                trace.record("stdout-broken-pipe", map([]));
                return spec.exit_code;
            }
        }
        if spec.out_of_order > 0 && staged.len() as u64 >= spec.out_of_order {
            // 攒满 N 条 → **逆序**吐出（乱序回帧路径）。
            staged.make_contiguous().reverse();
            if !flush(trace, out, &mut staged) {
                return spec.exit_code;
            }
        }

        if matches!(spec.behavior, Behavior::CloseStdout)
            && served >= spec.close_after_requests
            && out.is_open()
        {
            trace.record("close-stdout", map([("served", json!(served))]));
            out.close();
            // 关掉 stdout 之后仍然活着：要么等被杀，要么在 `--exit-after-ms` 之后
            // 以 `--exit-code` 退出（后者让"崩溃计数"有一条确定观察到 `Exited` 的路）。
            if spec.exit_after_ms > 0 {
                std::thread::sleep(Duration::from_millis(spec.exit_after_ms));
                trace.record("exit-after-close-stdout", map([]));
                return spec.exit_code;
            }
            while lines.next().is_some() {}
            return 0;
        }
    }
    // 循环结束前把攒下的回帧清干净（`reply_on_trigger` 场景里宿主可能没发控制帧）。
    flush(trace, out, &mut staged);
    spec.exit_code
}

/// 把攒下的回帧按当前顺序全部写出；返回是否**全部**写成功。
fn flush(trace: &mut Trace, out: &mut SidecarStdout, staged: &mut VecDeque<Vec<u8>>) -> bool {
    while let Some(frame) = staged.pop_front() {
        if !out.write_line(&frame) {
            trace.record("stdout-broken-pipe", map([]));
            return false;
        }
    }
    true
}

/// 正常回帧：回显 `callId` / `id` / `runtimeGeneration`，按 `--reply-error` 决定
/// 给 `result` 还是 `error`。
fn reply_frame(spec: &Spec, request: &Value) -> Vec<u8> {
    let mut frame = Map::new();
    frame.insert("jsonrpc".to_string(), json!("2.0"));
    frame.insert("id".to_string(), request.get("id").cloned().unwrap_or(Value::Null));
    frame.insert("callId".to_string(), request.get("callId").cloned().unwrap_or(Value::Null));
    frame.insert("method".to_string(), request.get("method").cloned().unwrap_or(Value::Null));
    let generation = match spec.force_generation {
        Some(gen) => json!(gen),
        None => request.get("runtimeGeneration").cloned().unwrap_or(Value::Null),
    };
    frame.insert("runtimeGeneration".to_string(), generation);
    match spec.reply_error_code {
        Some(code) => frame.insert(
            "error".to_string(),
            Value::Object(map([
                ("code", json!(code)),
                ("message", json!("tauron-test-sidecar 受控失败")),
            ])),
        ),
        None => frame.insert(
            "result".to_string(),
            Value::Object(map([
                ("sidecarPid", json!(std::process::id())),
                ("echo", request.clone()),
            ])),
        ),
    };
    serde_json::to_vec(&Value::Object(frame)).unwrap_or_else(|_| b"{}".to_vec())
}

/// `Oversize` 的那一条：一条**合法 JSON**、总长（含结尾 `\n`）恰好 `frame_bytes`
/// 字节、且大于 `tauron_proc::spawner::MAX_FRAME_BYTES` 的行帧。
///
/// 刻意让它**内容合法**：被拒的理由只能是"超过单帧字节上限"，不能混进
/// "JSON 解析失败"这个干扰项——对照用例把 `--frame-bytes` 调到上限以下，
/// 同一份构造代码就要能被 `ProcessFrameSinkImpl` 正常结算。
fn oversized_frame(request: &Value, frame_bytes: usize) -> Vec<u8> {
    // 用足够长的 pad 撑起来，再按目标长度裁掉 pad 的多余部分，逐次收敛到精确长度。
    let mut pad_len = frame_bytes.saturating_sub(1).saturating_add(64);
    loop {
        let candidate = json!({
            "jsonrpc": "2.0",
            "id": request.get("id").cloned().unwrap_or(Value::Null),
            "callId": request.get("callId").cloned().unwrap_or(Value::Null),
            "runtimeGeneration": request.get("runtimeGeneration").cloned().unwrap_or(Value::Null),
            "result": { "pad": "p".repeat(pad_len) },
        });
        let bytes = serde_json::to_vec(&candidate).unwrap_or_default();
        if bytes.len() + 1 == frame_bytes || pad_len == 0 {
            return bytes;
        }
        if bytes.len() + 1 > frame_bytes {
            pad_len -= bytes.len() + 1 - frame_bytes;
        } else {
            pad_len += frame_bytes - (bytes.len() + 1);
        }
    }
}
