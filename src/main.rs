// Shaicoin RandomX miner.
//
// The chain replaced its Hamiltonian-cycle proof of work with a vendored
// RandomX fork on 2026-09-01. This miner links that exact fork: a stock
// RandomX runs at full speed and produces hashes the chain has never seen.

mod ascii_art;
mod models;
mod hasher;
mod utils;
mod api;
mod randomx;

use utils::*;
use models::*;
use hasher::*;
use randomx::{RxKey, RxVm};
use rand::Rng;
use colored::*;
use std::thread;
use ascii_art::*;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use crate::api::MinerState;
use primitive_types::U256;
use futures_util::{StreamExt, SinkExt};
use std::sync::{atomic::{AtomicUsize, Ordering}, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

// The cache/dataset in use, plus a generation counter so workers can tell
// cheaply that the RandomX key rotated (every 2048 blocks, about 2.8 days).
struct KeyState { gen: u64, key: Option<Arc<RxKey>> }

#[tokio::main]
async fn main() {
    let args = Args::parse_and_validate();

    let max_workers = num_cpus::get();
    let num_workers = match args.threads {
        Some(t) if t > 0 && t <= max_workers => t,
        Some(_) => { println!("{}", "Thread count out of range; using all cores".bold().red()); max_workers }
        None => max_workers,
    };

    println!("{}", "STARTING MINER".bold().green());
    println!("{} {}", "USING WORKERS: ".bold().cyan(), format!("{}", num_workers).bold().cyan());
    if args.light {
        println!("{}", "LIGHT MODE: about 256 MB, roughly 10x slower than fast mode".bold().yellow());
    }
    print_startup_art();

    tokio::spawn(handle_exit_signals());

    let miner_id = args.address.clone().unwrap();
    let (server_sender, server_receiver) = mpsc::channel::<String>();
    let current_job: Arc<Mutex<Option<Job>>> = Arc::new(Mutex::new(None));
    let key_state = Arc::new(StdMutex::new(KeyState { gen: 0, key: None }));

    let miner_state = Arc::new(MinerState {
        hash_count: Arc::new(AtomicUsize::new(0)),
        accepted_shares: Arc::new(AtomicUsize::new(0)),
        rejected_shares: Arc::new(AtomicUsize::new(0)),
        hashrate_samples: Arc::new(Mutex::new(Vec::new())),
        version: String::from("3.0.0"),
    });

    let hash_count = Arc::new(AtomicUsize::new(0));
    for _ in 0..num_workers {
        let current_job = Arc::clone(&current_job);
        let key_state = Arc::clone(&key_state);
        let hash_count = Arc::clone(&hash_count);
        let api_hash_count = Arc::clone(&miner_state.hash_count);
        let sender = server_sender.clone();
        let miner_id = miner_id.clone();

        thread::spawn(move || {
            let mut vm: Option<RxVm> = None;
            let mut my_gen: u64 = u64::MAX;
            let mut nonce: u32 = rand::thread_rng().gen();

            loop {
                let job = match current_job.blocking_lock().clone() {
                    Some(j) => j,
                    None => { thread::sleep(Duration::from_millis(200)); continue; }
                };

                // Rebuild this worker VM if the key rotated.
                let (gen, key) = { let ks = key_state.lock().unwrap(); (ks.gen, ks.key.clone()) };
                if my_gen != gen || vm.is_none() {
                    vm = key.as_ref().and_then(|k| k.new_vm());
                    my_gen = gen;
                }
                let vm = match vm.as_mut() {
                    Some(v) => v,
                    None => { thread::sleep(Duration::from_millis(500)); continue; }
                };

                // Decode the header once, then only overwrite the 4 nonce bytes
                // per attempt, so there is no string work in the hot loop.
                let mut header = match hex::decode(&job.data) {
                    Ok(h) if h.len() >= HEADER_BYTES => h,
                    _ => { thread::sleep(Duration::from_millis(500)); continue; }
                };
                let target = match U256::from_str_radix(&job.target, 16) {
                    Ok(t) => t,
                    Err(_) => { thread::sleep(Duration::from_millis(500)); continue; }
                };

                // Grind this job until it changes.
                for i in 0u32.. {
                    nonce = nonce.wrapping_add(1);
                    let nb = nonce.to_le_bytes();
                    header[76..80].copy_from_slice(&nb);

                    let raw = pow_hash(vm, &header[..HEADER_BYTES]);
                    hash_count.fetch_add(1, Ordering::Relaxed);
                    api_hash_count.fetch_add(1, Ordering::Relaxed);

                    if hash_to_u256(&raw) <= target {
                        let nonce_hex: String = nb.iter().map(|b| format!("{:02x}", b)).collect();
                        let msg = SubmitMessage {
                            r#type: "submit".into(),
                            miner_id: miner_id.clone(),
                            nonce: nonce_hex,
                            job_id: job.job_id.clone(),
                        };
                        if let Ok(s) = serde_json::to_string(&msg) { let _ = sender.send(s); }
                    }

                    // Check for a new job periodically rather than locking on
                    // every single hash.
                    if i % 32 == 31 {
                        let changed = match current_job.blocking_lock().as_ref() {
                            Some(j) => j.job_id != job.job_id,
                            None => true,
                        };
                        if changed { break; }
                    }
                }
            }
        });
    }

    // Hashrate to stdout
    let hc = Arc::clone(&hash_count);
    tokio::spawn(async move {
        let mut last = 0usize;
        let mut t = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let c = hc.load(Ordering::Relaxed);
            let dt = t.elapsed().as_secs_f64();
            println!("{}: {:.1} H/s", "Hash rate".cyan(), (c - last) as f64 / dt);
            last = c; t = Instant::now();
        }
    });

    // Hashrate samples for the HTTP API
    let api_hr = miner_state.clone();
    tokio::spawn(async move {
        let mut last = 0usize;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let c = api_hr.hash_count.load(Ordering::Relaxed);
            let mut s = api_hr.hashrate_samples.lock().await;
            s.push((c - last) as u64);
            if s.len() > 10 { s.remove(0); }
            last = c;
        }
    });
    tokio::spawn(api::start_http_server(miner_state.clone()));

    let pool_url = args.pool.clone().unwrap();
    let server_receiver = Arc::new(Mutex::new(server_receiver));

    loop {
        let request = match pool_url.clone().into_client_request() {
            Ok(r) => r,
            Err(e) => { println!("{}", format!("Bad pool URL: {}", e).red()); std::process::exit(1); }
        };
        let ws_stream = match connect_async(request).await {
            Ok((s, _)) => { println!("{}", format!("Connected to {}", pool_url).bold().green()); s }
            Err(e) => {
                let d = rand::thread_rng().gen_range(5..30);
                println!("{}", format!("Connect failed ({}); retrying in {}s", e, d).red());
                tokio::time::sleep(Duration::from_secs(d)).await;
                continue;
            }
        };

        let (mut write, mut read) = ws_stream.split();
        let rx = Arc::clone(&server_receiver);
        let writer = tokio::spawn(async move {
            loop {
                let msg = { let g = rx.lock().await; g.recv() };
                match msg {
                    Ok(m) => { if write.send(Message::Text(m)).await.is_err() { break; } }
                    Err(_) => break,
                }
            }
        });

        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    let sm: ServerMessage = match serde_json::from_str(&text) { Ok(v) => v, Err(_) => continue };
                    match sm.r#type.as_str() {
                        "job" => {
                            let (Some(job_id), Some(data), Some(target)) = (sm.job_id, sm.data, sm.target) else { continue };
                            let Some(seed) = sm.seed else {
                                println!("{}", "Pool sent a job with no randomx seed; it has not been updated for the RandomX fork.".bold().red());
                                continue;
                            };
                            if data.len() < HEADER_HEX_LEN {
                                println!("{}", format!("Job header is {} hex chars, expected {}", data.len(), HEADER_HEX_LEN).red());
                                continue;
                            }

                            // Rebuild the cache/dataset only when the key rotates.
                            let need = { let ks = key_state.lock().unwrap();
                                ks.key.as_ref().map(|k| hex::encode(&k.seed) != seed).unwrap_or(true) };
                            if need {
                                let seed_bytes = match hex::decode(&seed) { Ok(b) => b, Err(_) => continue };
                                println!("{}", "RandomX key changed; building (this takes a moment)...".bold().yellow());
                                let fast = !args.light;
                                let nthreads = num_workers;
                                let built = tokio::task::spawn_blocking(move || RxKey::new(&seed_bytes, fast, nthreads)).await.ok().flatten();
                                match built {
                                    Some(k) => {
                                        println!("{}", format!("RandomX ready: {} mode, huge pages {}",
                                            if k.fast { "fast" } else { "light" },
                                            if k.large_pages { "ON" } else { "OFF, hashrate will be substantially lower" }).bold().green());
                                        let mut ks = key_state.lock().unwrap();
                                        ks.key = Some(k); ks.gen = ks.gen.wrapping_add(1);
                                    }
                                    None => { println!("{}", "Failed to allocate RandomX; try --light".bold().red()); continue; }
                                }
                            }

                            let mut g = current_job.lock().await;
                            *g = Some(Job { job_id: job_id.clone(), data, seed, target: target.clone() });
                            println!("{} {}", "New job".bold().blue(),
                                format!("id={} target={}...", job_id, &target[..target.len().min(16)]).yellow());
                        }
                        "accepted" => {
                            miner_state.accepted_shares.fetch_add(1, Ordering::Relaxed);
                            println!("{}", "Share accepted".bold().green());
                            display_share_accepted();
                        }
                        "rejected" => {
                            miner_state.rejected_shares.fetch_add(1, Ordering::Relaxed);
                            println!("{}", format!("Share rejected: {}", sm.message.unwrap_or_default()).red());
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => { println!("{}", "Connection closed.".red()); break; }
                Some(Err(_)) => { println!("{}", "Connection error.".red()); break; }
                _ => {}
            }
        }

        writer.abort();
        { let mut g = current_job.lock().await; *g = None; }
        let d = rand::thread_rng().gen_range(5..20);
        println!("{}", format!("Reconnecting in {}s...", d).yellow());
        tokio::time::sleep(Duration::from_secs(d)).await;
    }
}
