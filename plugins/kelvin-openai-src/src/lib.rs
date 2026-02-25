use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_OPENAI_MODEL: &str = "gpt-4.1-mini";

#[derive(Debug, Clone, Deserialize)]
struct ModelInput {
    run_id: String,
    session_id: String,
    system_prompt: String,
    user_prompt: String,
    memory_snippets: Vec<String>,
    history: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ModelUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct ToolCall {
    id: String,
    name: String,
    arguments: Value,
}

#[derive(Debug, Clone, Serialize)]
struct ModelOutput {
    assistant_text: String,
    stop_reason: Option<String>,
    tool_calls: Vec<ToolCall>,
    usage: Option<ModelUsage>,
}

#[link(wasm_import_module = "kelvin_model_host_v1")]
extern "C" {
    fn openai_responses_call(req_ptr: i32, req_len: i32) -> i64;
    fn log(level: i32, msg_ptr: i32, msg_len: i32) -> i32;
    fn clock_now_ms() -> i64;
}

#[no_mangle]
pub extern "C" fn alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    let len = len as usize;
    let mut bytes = Vec::<u8>::with_capacity(len);
    let ptr = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    ptr as i32
}

#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: i32, len: i32) {
    if ptr <= 0 || len <= 0 {
        return;
    }
    let ptr = ptr as *mut u8;
    let len = len as usize;
    let _ = Vec::from_raw_parts(ptr, 0, len);
}

#[no_mangle]
pub extern "C" fn infer(req_ptr: i32, req_len: i32) -> i64 {
    match infer_inner(req_ptr, req_len) {
        Ok(output) => pack_output_bytes(&output),
        Err(message) => {
            let error_payload = json!({
                "error": {
                    "message": message,
                }
            })
            .to_string();
            pack_output_bytes(error_payload.as_bytes())
        }
    }
}

fn infer_inner(req_ptr: i32, req_len: i32) -> Result<Vec<u8>, String> {
    let request_bytes = read_guest_bytes(req_ptr, req_len)?;
    let input: ModelInput = serde_json::from_slice(&request_bytes)
        .map_err(|err| format!("parse model input json: {err}"))?;

    host_log(1, &format!("kelvin.openai infer run_id={}", input.run_id));

    let user_text = build_user_text(&input);
    let request_payload = json!({
        "model": DEFAULT_OPENAI_MODEL,
        "input": [
            {
                "role": "system",
                "content": [{"type": "input_text", "text": input.system_prompt}]
            },
            {
                "role": "user",
                "content": [{"type": "input_text", "text": user_text}]
            }
        ],
        "metadata": {
            "run_id": input.run_id,
            "session_id": input.session_id,
            "generated_at_ms": unsafe { clock_now_ms() }
        }
    });

    let request_bytes = serde_json::to_vec(&request_payload)
        .map_err(|err| format!("serialize openai request json: {err}"))?;
    let packed_response = host_openai_call(&request_bytes)?;
    let (response_ptr, response_len) = unpack_ptr_len(packed_response)
        .ok_or_else(|| "invalid host response pointer/length".to_string())?;
    let response_bytes = read_guest_bytes(response_ptr, response_len)?;

    let response_json: Value = serde_json::from_slice(&response_bytes)
        .map_err(|err| format!("parse openai response json: {err}"))?;

    if let Some(message) = response_json
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
    {
        return Err(format!("openai error: {message}"));
    }

    let assistant_text = extract_assistant_text(&response_json);
    if assistant_text.trim().is_empty() {
        return Err("openai response did not contain assistant text".to_string());
    }

    let usage = response_json.get("usage").map(|usage| ModelUsage {
        input_tokens: usage.get("input_tokens").and_then(|v| v.as_u64()),
        output_tokens: usage.get("output_tokens").and_then(|v| v.as_u64()),
        total_tokens: usage.get("total_tokens").and_then(|v| v.as_u64()),
    });

    let output = ModelOutput {
        assistant_text,
        stop_reason: response_json
            .get("status")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or_else(|| Some("completed".to_string())),
        tool_calls: Vec::new(),
        usage,
    };

    serde_json::to_vec(&output).map_err(|err| format!("serialize model output json: {err}"))
}

fn build_user_text(input: &ModelInput) -> String {
    let mut sections = Vec::new();

    if !input.memory_snippets.is_empty() {
        sections.push(format!("Memory snippets:\n{}", input.memory_snippets.join("\n")));
    }

    if !input.history.is_empty() {
        sections.push(format!("History:\n{}", input.history.join("\n")));
    }

    sections.push(format!("User prompt:\n{}", input.user_prompt.trim()));

    sections.join("\n\n")
}

fn extract_assistant_text(response: &Value) -> String {
    if let Some(text) = response.get("output_text").and_then(|value| value.as_str()) {
        return text.to_string();
    }

    if let Some(output) = response.get("output").and_then(|value| value.as_array()) {
        let mut collected = Vec::new();
        for item in output {
            if let Some(content) = item.get("content").and_then(|value| value.as_array()) {
                for content_item in content {
                    if let Some(text) = content_item.get("text").and_then(|value| value.as_str()) {
                        collected.push(text.to_string());
                    }
                }
            }
        }
        if !collected.is_empty() {
            return collected.join("\n");
        }
    }

    String::new()
}

fn host_openai_call(request_bytes: &[u8]) -> Result<i64, String> {
    let req_len = i32::try_from(request_bytes.len())
        .map_err(|_| "openai request exceeded i32 address space".to_string())?;
    let req_ptr = alloc(req_len);
    if req_ptr <= 0 {
        return Err("failed to allocate request buffer".to_string());
    }
    write_guest_bytes(req_ptr, request_bytes)?;
    let packed = unsafe { openai_responses_call(req_ptr, req_len) };
    unsafe {
        dealloc(req_ptr, req_len);
    }
    Ok(packed)
}

fn pack_output_bytes(bytes: &[u8]) -> i64 {
    let len = match i32::try_from(bytes.len()) {
        Ok(value) => value,
        Err(_) => return 0,
    };
    if len <= 0 {
        return 0;
    }
    let ptr = alloc(len);
    if ptr <= 0 {
        return 0;
    }
    if write_guest_bytes(ptr, bytes).is_err() {
        unsafe {
            dealloc(ptr, len);
        }
        return 0;
    }
    pack_ptr_len(ptr, len)
}

fn read_guest_bytes(ptr: i32, len: i32) -> Result<Vec<u8>, String> {
    if ptr <= 0 || len < 0 {
        return Err("invalid pointer/length".to_string());
    }
    let len = len as usize;
    let src = ptr as *const u8;
    let mut out = vec![0_u8; len];
    unsafe {
        std::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), len);
    }
    Ok(out)
}

fn write_guest_bytes(ptr: i32, bytes: &[u8]) -> Result<(), String> {
    if ptr <= 0 {
        return Err("invalid write pointer".to_string());
    }
    let dst = ptr as *mut u8;
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
    }
    Ok(())
}

fn unpack_ptr_len(value: i64) -> Option<(i32, i32)> {
    if value <= 0 {
        return None;
    }
    let raw = value as u64;
    let ptr = (raw >> 32) as u32;
    let len = (raw & 0xFFFF_FFFF) as u32;
    if ptr == 0 {
        return None;
    }
    Some((ptr as i32, len as i32))
}

fn pack_ptr_len(ptr: i32, len: i32) -> i64 {
    let ptr_u32 = ptr as u32;
    let len_u32 = len as u32;
    ((u64::from(ptr_u32) << 32) | u64::from(len_u32)) as i64
}

fn host_log(level: i32, message: &str) {
    let message_bytes = message.as_bytes();
    let Ok(len) = i32::try_from(message_bytes.len()) else {
        return;
    };
    if len <= 0 {
        return;
    }
    let ptr = alloc(len);
    if ptr <= 0 {
        return;
    }
    if write_guest_bytes(ptr, message_bytes).is_ok() {
        unsafe {
            let _ = log(level, ptr, len);
        }
    }
    unsafe {
        dealloc(ptr, len);
    }
}
