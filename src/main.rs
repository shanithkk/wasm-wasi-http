use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use wasi_common::WasiCtx;
use wasmtime::*;
use wasmtime_wasi::sync::WasiCtxBuilder;
mod wasi_http;
use std::convert::TryInto;
use wasi_http::HttpCtx;

pub static WASM_FILE: &'static [u8] = include_bytes!("../wasm_wasi.wasm");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    allowed_hosts: Option<Vec<String>>,
    data: Value,
}

pub fn main() {
    let serde_json = serde_json::json!({
        "allowed_hosts": ["https://catfact.ninja/fact",
        ],
        "data": {
            "url": "https://catfact.ninja/fact",
        }

    });
    let input: Input = serde_json::from_value(serde_json).unwrap();

    let engine = Engine::default();
    let mut linker = Linker::new(&engine);

    let output: Arc<Mutex<Output>> = Arc::new(Mutex::new(Output {
        result: serde_json::json!({}),
    }));
    let output_ = output.clone();
    let buf = serde_json::to_vec(&input).expect("should serialize");
    let mem_size: i32 = buf.len() as i32;

    linker
        .func_wrap("host", "get_input_size", move || -> i32 { mem_size })
        .expect("should define the function");

    linker
        .func_wrap(
            "host",
            "set_output",
            move |mut caller: Caller<'_, WasiCtx>, ptr: i32, capacity: i32| {
                let output = output_.clone();
                let mem = match caller.get_export("memory") {
                    Some(Extern::Memory(mem)) => mem,
                    _ => return Err(Trap::new("failed to find host memory")),
                };
                let offset = ptr as u32 as usize;
                let mut buffer: Vec<u8> = vec![0; capacity as usize];
                match mem.read(&caller, offset, &mut buffer) {
                    Ok(()) => {
                        println!(
                            "Buffer = {:?}, ptr = {}, capacity = {}",
                            buffer, ptr, capacity
                        );
                        match serde_json::from_slice::<Output>(&buffer) {
                            Ok(serialized_output) => {
                                let mut output = output.lock().unwrap();
                                *output = serialized_output;
                                Ok(())
                            }
                            Err(err) => {
                                let msg = format!("failed to serialize host memory: {}", err);
                                Err(Trap::new(msg))
                            }
                        }
                    }
                    _ => Err(Trap::new("failed to read host memory")),
                }
            },
        )
        .expect("should define the function");

    wasmtime_wasi::add_to_linker(&mut linker, |s| s).unwrap();
    let wasi = WasiCtxBuilder::new()
        .inherit_stdio()
        .inherit_args()
        .unwrap()
        .build();
    let mut store = Store::new(&engine, wasi);
    let module = Module::from_binary(&engine, WASM_FILE).unwrap();
    let max_concurrent_requests = Some(42);

    let http = HttpCtx::new(input.allowed_hosts, max_concurrent_requests).unwrap();
    http.add_to_linker(&mut linker).unwrap();

    let linking = linker.instantiate(&mut store, &module).unwrap();

    let malloc = linking
        .get_typed_func::<(i32, i32), i32, _>(&mut store, "memory_alloc")
        .unwrap();
    let data = serde_json::to_vec(&input.data).unwrap();
    let data_ptr = malloc.call(&mut store, (data.len() as i32, 2)).unwrap();

    let memory = linking.get_memory(&mut store, "memory").unwrap();
    memory.write(&mut store, data_ptr as usize, &data).unwrap();
    let len: i32 = data.len().try_into().unwrap();
    let run = linking
        .get_typed_func::<(i32, i32), (), _>(&mut store, "_start")
        .unwrap();
    let _result_from_wasm = run.call(&mut store, (data_ptr, len));

    let malloc = linking
        .get_typed_func::<(i32, i32, i32), (), _>(&mut store, "free_memory")
        .unwrap();
    malloc
        .call(&mut store, (data_ptr, data.len() as i32, 2))
        .unwrap();
    let output = output.lock();
    println!("{}", serde_json::json!({"result": output.unwrap().result}));
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Output {
    pub result: Value,
}
