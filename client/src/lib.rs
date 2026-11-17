use gloo_net::http::Request;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::{Blob, FormData, HtmlInputElement};

const API_BASE: &str = "http://localhost:3000";
/// サーバーの本文制限と同じ 10 MiB。選択時に拒否して無駄な送信を避ける。
const MAX_UPLOAD_BYTES: f64 = 10.0 * 1024.0 * 1024.0;

fn render_markdown(markdown: &str) -> String {
    let events = pulldown_cmark::Parser::new(markdown).map(|event| match event {
        pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link {
            link_type,
            dest_url: if is_safe_markdown_url(&dest_url, false) {
                dest_url
            } else {
                "#".into()
            },
            title,
            id,
        }),
        pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image {
            link_type,
            dest_url: if is_safe_markdown_url(&dest_url, true) {
                dest_url
            } else {
                "#".into()
            },
            title,
            id,
        }),
        pulldown_cmark::Event::Html(html) | pulldown_cmark::Event::InlineHtml(html) => {
            pulldown_cmark::Event::Text(html)
        }
        event => event,
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    html
}

/// Markdown links and images may only use web-safe schemes or relative destinations.
fn is_safe_markdown_url(destination: &str, image: bool) -> bool {
    // ASCII に限らず Unicode の空白・制御文字を拒否する。
    // これらを先頭に付けるとスキーム解析が外れ、ブラウザが javascript: を
    // 実行するバイパスになり得る（ZWSP / NBSP 等）。
    if destination
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return false;
    }

    let Some((scheme, _)) = destination.split_once(':') else {
        return true;
    };
    // `:` があるのにスキームが不正な場合は、相対 URL として扱わず拒否する
    // （fail-closed）。曖昧な文字列を安全とみなさない。
    if scheme.is_empty()
        || !scheme.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'A'..=b'Z' => true,
            b'0'..=b'9' | b'+' | b'.' | b'-' => index > 0,
            _ => false,
        })
    {
        return false;
    }

    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => true,
        "mailto" => !image,
        _ => false,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub authorized: bool,
    pub user_hash: Option<String>,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Oid4vpRequest {
    nonce: String,
    #[allow(dead_code)]
    presentation_definition_uri: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentMeta {
    pub cid: String,
    pub checksum: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub status: String,
    pub price: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub tx_id: i64,
    pub created_at: String,
    pub sender_hash: String,
    pub receiver_hash: String,
    pub amount: i64,
    pub tx_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditLog {
    pub log_id: i64,
    pub old_status: Option<String>,
    pub new_status: String,
    pub changed_by: String,
    pub reason: Option<String>,
    pub created_at: String,
    pub entry_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub report_id: i64,
    pub target_cid: String,
    pub reason: String,
    pub created_at: String,
    pub status: String,
}

#[component]
pub fn App() -> impl IntoView {
    let (view, set_view) = signal("login".to_string());
    let (user_hash, set_user_hash) = signal(None::<String>);
    let (token, set_token) = signal(None::<String>);

    let handle_login_success = move |hash: String, jwt: String| {
        set_user_hash.set(Some(hash));
        set_token.set(Some(jwt));
        set_view.set("dashboard".to_string());
    };

    view! {
        <div class="min-h-screen bg-black text-white font-mono">
            <nav class="border-b border-white/10 p-4 flex justify-between items-center sticky top-0 bg-black z-50">
                <h1 class="text-xl font-bold tracking-tighter">OPENILLUSTRATION / GUILD (RUST)</h1>
                <Show when=move || view.get() != "login" fallback=|| ()>
                    <div class="flex gap-6 text-[10px] tracking-widest font-bold">
                        <button on:click=move |_| set_view.set("dashboard".to_string())
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "dashboard" { "text-white" } else { "text-gray-500" })>
                            "BROWSE"
                        </button>
                        <button on:click=move |_| set_view.set("upload".to_string())
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "upload" { "text-white" } else { "text-gray-500" })>
                            "UPLOAD"
                        </button>
                        <button on:click=move |_| set_view.set("moderation".to_string())
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "moderation" { "text-white" } else { "text-gray-500" })>
                            "MODERATION"
                        </button>
                        <button on:click=move |_| set_view.set("ledger".to_string())
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "ledger" { "text-white" } else { "text-gray-500" })>
                            "LEDGER"
                        </button>
                        <span class="text-green-500 border border-green-500/20 px-2">
                            {move || user_hash.get().map(|h| h[0..8.min(h.len())].to_string()).unwrap_or_default()}
                        </span>
                    </div>
                </Show>
            </nav>

            <main class="container mx-auto p-4">
                {move || match view.get().as_str() {
                    "login" => view! { <Login on_success=handle_login_success /> }.into_any(),
                    "dashboard" => view! { <Dashboard token=token.get().unwrap_or_default() user_hash=user_hash.get().unwrap_or_default() /> }.into_any(),
                    "upload" => view! { <Upload user_hash=user_hash.get().unwrap_or_default() token=token.get().unwrap_or_default() /> }.into_any(),
                    "moderation" => view! {
                        <Moderation token=token.get().unwrap_or_default() user_hash=user_hash.get().unwrap_or_default() />
                    }.into_any(),
                    "ledger" => view! {
                        <Ledger token=token.get().unwrap_or_default() />
                    }.into_any(),
                    _ => view! { <div class="text-center mt-20 text-gray-600">"UNKNOWN_VIEW"</div> }.into_any(),
                }}
            </main>
        </div>
    }
}

#[component]
fn Login<F>(on_success: F) -> impl IntoView
where
    F: Fn(String, String) + 'static + Clone,
{
    let (loading, set_loading) = signal(false);

    let do_login = move |_| {
        set_loading.set(true);
        let on_success = on_success.clone();
        spawn_local(async move {
            // 開発用モック認証でも nonce を取得して単回消費のフローを通す。
            let nonce = match Request::post(&format!("{}/auth/login", API_BASE))
                .send()
                .await
            {
                Ok(response) => response.json::<Oid4vpRequest>().await.ok().map(|s| s.nonce),
                Err(_) => None,
            };
            let res = Request::post(&format!("{}/auth/verify", API_BASE))
                .json(&serde_json::json!({
                    "presentation_token": "mock_token",
                    "nonce": nonce,
                    "birth_date": "2000-01-01",
                    "country_code": "JP"
                }))
                .unwrap()
                .send()
                .await;

            if let Ok(response) = res
                && let Ok(data) = response.json::<VerifyResponse>().await
                && data.authorized
                && let (Some(hash), Some(jwt)) = (data.user_hash, data.token)
            {
                on_success(hash, jwt);
            }
            set_loading.set(false);
        });
    };

    view! {
        <div class="max-w-md mx-auto mt-20 border border-white/10 p-8 text-center">
            <h2 class="text-2xl font-bold mb-8 tracking-tighter italic">"IDENTITY_VERIFICATION_REQUIRED"</h2>
            <p class="text-[10px] text-gray-500 mb-4 leading-relaxed">
                "IPv6-ONLY ACCESS: Legacy IPv4-only networks are rejected at the API layer (SEC-01)."
            </p>
            <p class="text-[10px] text-gray-500 mb-8 leading-relaxed">
                "JPKI-BASED AGE AND IDENTITY VERIFICATION. PERSONAL DATA IS HASHED ONLY."
            </p>
            <button on:click=do_login disabled=loading
                class="w-full py-4 border border-white hover:bg-white hover:text-black transition-all font-bold uppercase tracking-widest disabled:opacity-50">
                {move || if loading.get() { "VERIFYING..." } else { "CONNECT WITH JPKI" }}
            </button>
        </div>
    }
}

#[component]
fn Dashboard(token: String, user_hash: String) -> impl IntoView {
    let (items, set_items) = signal(Vec::<ContentMeta>::new());
    let (error, set_error) = signal(String::new());

    let token_stored = StoredValue::new(token);
    let user_hash_stored = StoredValue::new(user_hash);

    Effect::new(move |_| {
        let token = token_stored.get_value();
        spawn_local(async move {
            match Request::get(&format!("{}/content", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
                .await
            {
                Ok(res) if res.ok() => {
                    if let Ok(list) = res.json::<Vec<ContentMeta>>().await {
                        set_items.set(list);
                    }
                }
                Ok(_) => set_error.set("Failed to load content index.".to_string()),
                Err(_) => set_error.set("Network error loading content.".to_string()),
            }
        });
    });

    view! {
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-8 mt-8">
            <For
                each=move || items.get()
                key=|item| item.cid.clone()
                children=move |item| {
                    view! {
                        <ContentViewer
                            cid=item.cid.clone()
                            expected_checksum=item.checksum.clone()
                            content_type=item.content_type.clone()
                            price=item.price
                            token=token_stored.get_value()
                            user_hash=user_hash_stored.get_value()
                        />
                    }
                }
            />
            <Show when=move || items.get().is_empty() fallback=|| ()>
                <div class="border border-white/10 p-4 opacity-50 flex items-center justify-center italic text-[10px] text-gray-600 col-span-full">
                    {move || if error.get().is_empty() {
                        "NO_APPROVED_CONTENT".to_string()
                    } else {
                        error.get()
                    }}
                </div>
            </Show>
        </div>
    }
}

mod jxl;

#[component]
fn ContentViewer(
    cid: String,
    expected_checksum: String,
    content_type: String,
    price: i64,
    token: String,
    #[allow(unused_variables)] user_hash: String,
) -> impl IntoView {
    let cid_label = cid.clone();
    let (verified, set_verified) = signal(None::<bool>);
    let (novel_html, set_novel_html) = signal(String::new());
    let (audit_logs, set_audit_logs) = signal(Vec::<AuditLog>::new());
    let (show_audit, set_show_audit) = signal(false);
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    let token_stored = StoredValue::new(token);

    let ct_ef = content_type.clone();
    Effect::new(move |_| {
        let cid = cid.clone();
        let expected_checksum = expected_checksum.clone();
        let ct = ct_ef.clone();
        let token = token_stored.get_value();
        spawn_local(async move {
            let url = format!("{}/content/{}", API_BASE, cid);
            let res = Request::get(&url)
                .header("Authorization", &format!("Bearer {}", token))
                .send()
                .await;

            if let Ok(response) = res {
                let bytes = response.binary().await.unwrap_or_default();
                let is_ok = common::verify_checksum(&bytes, &expected_checksum);
                set_verified.set(Some(is_ok));

                if is_ok && !bytes.is_empty() {
                    if ct == "Illustration" {
                        if let Some(canvas) = canvas_ref.get() {
                            use wasm_bindgen::JsCast;
                            let el: web_sys::HtmlCanvasElement = (*canvas).clone().unchecked_into();
                            let _ = jxl::decode_jxl_to_canvas(&bytes, el);
                        }
                    } else {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        set_novel_html.set(render_markdown(&text));
                    }
                }
            } else {
                set_verified.set(Some(false));
            }

            if let Ok(audit_res) = Request::get(&format!("{}/content/{}/audit", API_BASE, cid))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
                .await
                && let Ok(logs) = audit_res.json::<Vec<AuditLog>>().await
            {
                set_audit_logs.set(logs);
            }
        });
    });

    let ct_can = content_type.clone();
    let ct_nov = content_type.clone();
    let ct_bad = content_type.clone();

    view! {
        <div class="p-4 border border-white/10 bg-black text-white space-y-4 flex flex-col h-full">
            <div class="flex justify-between items-start">
                <p class="text-[8px] text-gray-600 truncate flex-1 mr-2">{cid_label.clone()}</p>
                <div class="flex gap-2">
                    <button on:click=move |_| set_show_audit.update(|v| *v = !*v)
                        class="text-[8px] border border-white/20 px-1 hover:bg-white/10 transition-colors uppercase">
                        "Audit"
                    </button>
                </div>
            </div>

            <div class="aspect-square bg-white/5 flex items-center justify-center relative overflow-hidden group">
                <canvas node_ref=canvas_ref class=move || format!("max-w-full max-h-full object-contain {}", if verified.get() == Some(true) && ct_can == "Illustration" { "" } else { "hidden" }) />

                <Show when=move || verified.get() == Some(true) && ct_nov == "Novel" fallback=|| ()>
                    <div class="p-4 text-xs prose prose-invert max-w-none w-full overflow-y-auto h-full" inner_html=move || novel_html.get() />
                </Show>

                {move || match verified.get() {
                    Some(true) => view! {
                        <div class="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 transition-opacity flex flex-col items-center justify-center p-6 text-center space-y-4">
                            <p class="text-[10px] font-bold tracking-widest text-white uppercase">"Support Creator"</p>
                            <p class="text-[20px] font-bold text-green-500 font-mono">{price} " JPYC"</p>
                            <button disabled
                                class="px-6 py-2 border border-white/20 text-gray-500 cursor-not-allowed font-bold text-[10px] uppercase">
                                "Settlement Pending"
                            </button>
                            <p class="text-[8px] text-gray-400 italic">"JPYC settlement verification is in development. Support is disabled until then."</p>
                        </div>
                    }.into_any(),
                    Some(false) => view! {
                        <div class="text-center p-4">
                            <p class="text-red-500 font-bold text-[10px] mb-2 uppercase">"Integrity Breach Detected"</p>
                            <p class="text-gray-500 text-[8px]">"Wasm checksum does not match forensic registry."</p>
                        </div>
                    }.into_any(),
                    None => view! { <span class="text-[10px] text-gray-600 animate-pulse uppercase tracking-tighter">"Verifying_Integrity..."</span> }.into_any(),
                }}

                <Show when=move || show_audit.get() fallback=|| ()>
                    <div class="absolute inset-0 bg-black/95 z-20 p-4 overflow-y-auto border border-white/20">
                        <h4 class="text-[10px] font-bold mb-4 border-b border-white/10 pb-2">"FORENSIC_TIMELINE"</h4>
                        <div class="space-y-4">
                            <For
                                each=move || audit_logs.get()
                                key=|log| log.log_id
                                children=move |log| {
                                    view! {
                                        <div class="text-[8px] space-y-1">
                                            <div class="flex justify-between text-gray-500">
                                                <span>{log.created_at.clone()}</span>
                                                <span class="text-green-500">{log.new_status.clone()}</span>
                                            </div>
                                            <p class="text-gray-300">"By: " {log.changed_by.clone()}</p>
                                            <p class="italic">"Reason: " {log.reason.clone().unwrap_or_default()}</p>
                                            <p class="text-[6px] text-gray-700 break-all font-mono">"Hash: " {log.entry_hash.clone()}</p>
                                        </div>
                                    }
                                }
                            />
                        </div>
                        <button on:click=move |_| set_show_audit.set(false)
                            class="mt-4 w-full py-2 border border-white/20 text-[8px] hover:bg-white/10">
                            "CLOSE"
                        </button>
                    </div>
                </Show>
            </div>
            <div class="text-[10px] tracking-widest font-bold mt-auto flex justify-between items-center">
                <div class="flex items-center gap-4">
                    <span class=move || match verified.get() {
                        Some(true) => "text-green-500",
                        Some(false) => "text-red-500",
                        None => "text-gray-600",
                    }>
                        {move || match verified.get() {
                            Some(true) => "● VERIFIED",
                            Some(false) => "× COMPROMISED",
                            None => "○ PENDING",
                        }}
                    </span>
                    <span class="text-gray-500 border border-gray-500/20 px-1">{ct_bad.to_uppercase()}</span>
                </div>
                <span class="text-green-500 font-mono">{price} " JPYC"</span>
            </div>
        </div>
    }
}

#[component]
fn Upload(user_hash: String, token: String) -> impl IntoView {
    let (status, set_status) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (file, set_file) = signal(None::<web_sys::File>);
    let (price, set_price) = signal(100);

    let on_file_change = move |ev| {
        let input: HtmlInputElement = event_target(&ev);
        if let Some(files) = input.files()
            && let Some(f) = files.get(0)
        {
            if f.size() > MAX_UPLOAD_BYTES {
                set_file.set(None);
                set_status.set("ERROR: File exceeds the 10 MiB limit.".to_string());
                return;
            }
            set_status.set(String::new());
            set_file.set(Some(f));
        }
    };

    let sign_and_upload = move |_| {
        // ファイル未選択（サイズ拒否を含む）ではパニックせずエラー表示する。
        let Some(f) = file.get() else {
            set_status.set("ERROR: Select a file first.".to_string());
            return;
        };
        let user_hash = user_hash.clone();
        let token = token.clone();
        let price_val = price.get();
        set_loading.set(true);
        set_status.set("Generating forensic evidence...".to_string());

        spawn_local(async move {
            let array_buffer = wasm_bindgen_futures::JsFuture::from(f.array_buffer())
                .await
                .unwrap();
            let bytes = js_sys::Uint8Array::new(&array_buffer).to_vec();

            let mock_signature = vec![0u8; 256];

            let form_data = FormData::new().unwrap();
            let blob = Blob::new_with_u8_array_sequence(&js_sys::Array::of1(
                &js_sys::Uint8Array::from(&bytes[..]).into(),
            ))
            .unwrap();
            form_data
                .append_with_blob_and_filename("file", &blob, "upload.jxl")
                .unwrap();
            form_data.append_with_str("owner_hash", &user_hash).unwrap();
            form_data
                .append_with_str("price", &price_val.to_string())
                .unwrap();
            let sig_blob = Blob::new_with_u8_array_sequence(&js_sys::Array::of1(
                &js_sys::Uint8Array::from(&mock_signature[..]).into(),
            ))
            .unwrap();
            form_data.append_with_blob("signature", &sig_blob).unwrap();

            let res = Request::post(&format!("{}/content/upload", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .body(form_data)
                .unwrap()
                .send()
                .await;

            if let Ok(response) = res {
                if response.ok() {
                    if let Ok(cid) = response.json::<String>().await {
                        set_status.set(format!("SUCCESS: Content registered as {}", cid));
                    }
                } else {
                    set_status.set("ERROR: Upload failed.".to_string());
                }
            }
            set_loading.set(false);
        });
    };

    view! {
        <div class="max-w-2xl mx-auto mt-12 border border-white/10 p-8">
            <h2 class="text-xl font-bold mb-6 tracking-widest uppercase">"Forensic Upload Engine (Rust)"</h2>
            <div class="space-y-6">
                <div class="border-2 border-dashed border-white/20 p-12 text-center hover:border-white/40 transition-colors">
                    <input type="file" on:change=on_file_change class="hidden" id="file-upload" />
                    <label for="file-upload" class="cursor-pointer">
                        {move || file.get().map(|f| f.name()).unwrap_or_else(|| "SELECT JXL OR MARKDOWN".to_string())}
                    </label>
                </div>

                <div class="space-y-2">
                    <label class="text-[10px] text-gray-500 uppercase tracking-widest">"Support Price (JPYC)"</label>
                    <input type="number"
                        on:input=move |ev| set_price.set(event_target_value(&ev).parse().unwrap_or(0))
                        prop:value=price
                        class="w-full bg-black border border-white/20 p-4 text-xl font-mono text-green-500" />
                </div>

                <button on:click=sign_and_upload disabled=move || file.get().is_none() || loading.get()
                    class="w-full py-4 bg-white text-black font-bold uppercase tracking-widest hover:bg-gray-200 transition-colors disabled:opacity-50">
                    {move || if loading.get() { "Processing..." } else { "Sign & Submit to Evidence Ledger" }}
                </button>
                <Show when=move || !status.get().is_empty() fallback=|| ()>
                    <div class="p-4 bg-white/5 border border-white/20 text-xs">
                        <span class="text-gray-500 mr-2">"[STATUS]"</span> {move || status.get()}
                    </div>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn Ledger(token: String) -> impl IntoView {
    let (txs, set_txs) = signal(Vec::<Transaction>::new());

    Effect::new(move |_| {
        let token = token.clone();
        spawn_local(async move {
            let res = Request::get(&format!("{}/transactions", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
                .await;
            if let Ok(response) = res
                && let Ok(data) = response.json::<Vec<Transaction>>().await
            {
                set_txs.set(data);
            }
        });
    });

    view! {
        <div class="max-w-5xl mx-auto mt-12">
            <h2 class="text-xl font-bold tracking-widest uppercase mb-8">"Forensic Transaction Ledger"</h2>
            <div class="border border-amber-500/30 bg-amber-500/5 text-amber-300/90 text-[9px] p-3 mb-4 font-mono leading-relaxed">
                "LEGACY MOCK RECORDS — Written by the disabled mock support API. These entries are not proof of payment; JPYC settlement is not implemented."
            </div>
            <div class="border border-white/10 overflow-hidden">
                <table class="w-full text-left text-[10px] font-mono">
                    <thead class="bg-white/5 border-b border-white/10 uppercase tracking-tighter">
                        <tr>
                            <th class="p-4">"Date"</th>
                            <th class="p-4">"Sender (P2P)"</th>
                            <th class="p-4">"Receiver"</th>
                            <th class="p-4 text-right">"Amount"</th>
                            <th class="p-4">"Record_Hash"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-white/5">
                        <For
                            each=move || txs.get()
                            key=|tx| tx.tx_id
                            children=move |tx| {
                                let sender_prefix = tx.sender_hash[0..12.min(tx.sender_hash.len())].to_string();
                                let receiver_prefix = tx.receiver_hash[0..12.min(tx.receiver_hash.len())].to_string();
                                view! {
                                    <tr class="hover:bg-white/5 transition-colors">
                                        <td class="p-4 text-gray-500">{tx.created_at}</td>
                                        <td class="p-4 text-blue-400">{sender_prefix} "..."</td>
                                        <td class="p-4 text-green-400">{receiver_prefix} "..."</td>
                                        <td class="p-4 text-right font-bold">{tx.amount} " JPYC"</td>
                                        <td class="p-4 text-gray-600 text-[6px] break-all max-w-[200px]">{tx.tx_hash}</td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
                <Show when=move || txs.get().is_empty() fallback=|| ()>
                    <div class="p-12 text-center text-gray-600 italic">"NO_TRANSACTIONS_RECORDED"</div>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn Moderation(token: String, user_hash: String) -> impl IntoView {
    let (cid, set_cid) = signal(String::new());
    let (reason, set_reason) = signal(String::new());
    let (status, set_status) = signal(String::new());
    let (reports, set_reports) = signal(Vec::<Report>::new());

    let token_stored = StoredValue::new(token);
    let user_hash_stored = StoredValue::new(user_hash);

    let fetch_reports = move || {
        let token = token_stored.get_value();
        spawn_local(async move {
            let res = Request::get(&format!("{}/mod/reports", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
                .await;
            if let Ok(response) = res
                && let Ok(data) = response.json::<Vec<Report>>().await
            {
                set_reports.set(data);
            }
        });
    };

    Effect::new(move |_| {
        fetch_reports();
    });

    let submit_report = move |_| {
        let cid = cid.get();
        let reason = reason.get();
        let token = token_stored.get_value();
        set_status.set("Submitting report...".to_string());
        spawn_local(async move {
            let res = Request::post(&format!("{}/mod/report", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .json(&serde_json::json!({
                    "report_id": 0,
                    "target_cid": cid,
                    "reason": reason,
                    "status": "Pending"
                }))
                .unwrap()
                .send()
                .await;
            if let Ok(r) = res
                && r.ok()
            {
                set_status.set("Report accepted. Content marked Reported.".to_string());
                fetch_reports();
            } else {
                set_status.set("Report failed.".to_string());
            }
        });
    };

    let do_vote = move |report_id: i64, result: String| {
        let token = token_stored.get_value();
        let user_hash = user_hash_stored.get_value();
        spawn_local(async move {
            let mock_signature = vec![0u8; 256];
            let res = Request::post(&format!("{}/mod/vote", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .json(&serde_json::json!({
                    "report_id": report_id,
                    "juror_hash": user_hash,
                    "result": result,
                    "signature": mock_signature
                }))
                .unwrap()
                .send()
                .await;
            if let Ok(r) = res
                && r.ok()
            {
                fetch_reports();
            }
        });
    };

    view! {
        <div class="max-w-4xl mx-auto mt-12 grid grid-cols-1 md:grid-cols-2 gap-12">
            <div class="border border-white/10 p-8 space-y-6 h-fit">
                <h2 class="text-xl font-bold tracking-widest uppercase">"Submit Report"</h2>
                <p class="text-[10px] text-gray-500">
                    "Reporter: " {move || user_hash_stored.get_value()[0..8.min(user_hash_stored.get_value().len())].to_string()}
                </p>
                <input
                    type="text"
                    placeholder="TARGET CID"
                    on:input=move |ev| set_cid.set(event_target_value(&ev))
                    class="w-full bg-black border border-white/20 p-3 text-sm"
                />
                <textarea
                    placeholder="REASON"
                    on:input=move |ev| set_reason.set(event_target_value(&ev))
                    class="w-full bg-black border border-white/20 p-3 text-sm h-24"
                />
                <button on:click=submit_report
                    class="w-full py-3 border border-white hover:bg-white hover:text-black font-bold text-xs tracking-widest">
                    "SUBMIT FORENSIC REPORT"
                </button>
                <Show when=move || !status.get().is_empty() fallback=|| ()>
                    <p class="text-xs text-gray-400">{move || status.get()}</p>
                </Show>
            </div>

            <div class="border border-white/10 p-8 space-y-6">
                <h2 class="text-xl font-bold tracking-widest uppercase">"Jury Voting Queue"</h2>
                <div class="space-y-4">
                    <For
                        each=move || reports.get()
                        key=|report| report.report_id
                        children=move |report| {
                            let rid = report.report_id;
                            view! {
                                <div class="border border-white/5 p-4 space-y-3">
                                    <div class="flex justify-between items-center">
                                        <span class="text-[8px] text-gray-500 truncate mr-2">{report.target_cid.clone()}</span>
                                        <span class="text-[8px] border border-red-500/20 text-red-500 px-1 uppercase">"Reported"</span>
                                    </div>
                                    <p class="text-[10px] text-gray-300 italic">"Reason: " {report.reason.clone()}</p>
                                    <div class="grid grid-cols-2 gap-4">
                                        <button on:click=move |_| do_vote(rid, "Approve".to_string())
                                            class="py-2 border border-green-500/50 text-green-500 text-[8px] hover:bg-green-500 hover:text-black transition-colors">
                                            "APPROVE (RESTORE)"
                                        </button>
                                        <button on:click=move |_| do_vote(rid, "Reject".to_string())
                                            class="py-2 border border-red-500/50 text-red-500 text-[8px] hover:bg-red-500 hover:text-black transition-colors">
                                            "REJECT (BAN)"
                                        </button>
                                    </div>
                                </div>
                            }
                        }
                    />
                    <Show when=move || reports.get().is_empty() fallback=|| ()>
                        <div class="text-center py-12 text-[10px] text-gray-600 italic">"NO_PENDING_VOTES"</div>
                    </Show>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn markdown_raw_html_is_escaped() {
        let rendered =
            super::render_markdown("<img src=x onerror=alert(1)>\n\n<script>alert(1)</script>");

        assert!(!rendered.contains("<img"));
        assert!(!rendered.contains("<script>"));
        assert!(rendered.contains("&lt;img"));
        assert!(rendered.contains("&lt;script&gt;"));
    }

    #[test]
    fn markdown_unsafe_link_and_image_schemes_are_neutralized() {
        let rendered = super::render_markdown(
            "[script](javascript:alert%281%29) [mixed](JaVaScRiPt:alert%281%29) [data](data:text/html,alert) ![svg](data:image/svg+xml,%3Csvg%20onload=alert%281%29%3E)",
        );

        assert!(!rendered.contains("href=\"javascript:"));
        assert!(!rendered.contains("href=\"JaVaScRiPt:"));
        assert!(!rendered.contains("href=\"data:"));
        assert!(!rendered.contains("src=\"data:"));
        assert!(rendered.contains("href=\"#\""));
        assert!(rendered.contains("src=\"#\""));
        assert!(!super::is_safe_markdown_url(" javascript:alert(1)", false));
        assert!(!super::is_safe_markdown_url("java\tscript:alert(1)", false));
        // Unicode の空白制御文字でスキームを隠すバイパスを拒否する。
        assert!(!super::is_safe_markdown_url(
            "\u{200B}javascript:alert(1)",
            false
        ));
        assert!(!super::is_safe_markdown_url(
            "\u{00A0}javascript:alert(1)",
            false
        ));

        // レンダリング経由でも危険な href/src が残らないことを確認する。
        let rendered = super::render_markdown(
            "[\u{200B}js](javascript:alert%281%29) ![\u{200B}img](javascript:alert%281%29)",
        );
        assert!(!rendered.contains("href=\"javascript:"));
        assert!(!rendered.contains("src=\"javascript:"));
        assert!(rendered.contains("href=\"#\""));
        assert!(rendered.contains("src=\"#\""));
    }

    #[test]
    fn markdown_safe_link_schemes_and_relative_urls_are_preserved() {
        let rendered = super::render_markdown(
            "[secure](https://example.com) [email](mailto:user@example.com) [relative](../chapter/1) [fragment](#section)",
        );

        assert!(rendered.contains("href=\"https://example.com\""));
        assert!(rendered.contains("href=\"mailto:user@example.com\""));
        assert!(rendered.contains("href=\"../chapter/1\""));
        assert!(rendered.contains("href=\"#section\""));
    }

    /// 参照リンク定義・自動リンク・実体参照で難読化したスキームも中立化する。
    /// これらはすべて `Tag::Link` / `Tag::Image` として解決されるため、
    /// インライン記法と同じ検査を通る。
    #[test]
    fn markdown_reference_autolink_and_entity_schemes_are_neutralized() {
        let rendered = super::render_markdown(
            "[ref][target] ![badge][badge-src] [entity](javascript&#58;alert%281%29)\n\n<data:text/html,alert>\n\n[rel][chapter]\n\n[target]: javascript:alert(1)\n[badge-src]: data:image/svg+xml,%3Csvg%20onload=alert%281%29%3E\n[chapter]: /chapter/1\n",
        );

        assert!(!rendered.contains("href=\"javascript:"));
        assert!(!rendered.contains("href=\"data:"));
        assert!(!rendered.contains("src=\"data:"));
        // 参照リンク・実体参照リンク・自動リンクの3件が中立化される。
        assert_eq!(rendered.matches("href=\"#\"").count(), 3, "{rendered}");
        assert_eq!(rendered.matches("src=\"#\"").count(), 1, "{rendered}");
        assert!(rendered.contains("href=\"/chapter/1\""), "{rendered}");
    }

    #[test]
    fn wasm_checksum_matches_common_crate() {
        let data = b"guild forensic payload";
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(data);
        let hex = hex::encode(hasher.finalize());
        assert!(common::verify_checksum(data, &hex));
    }

    /// INT-01: 改ざんされたコンテンツのチェックサム検知
    #[test]
    fn wasm_detects_tampered_content() {
        let original_data = b"original forensic content";
        let tampered_data = b"tampered forensic content";

        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(original_data);
        let original_checksum = hex::encode(hasher.finalize());

        // 改ざんされたデータは元のチェックサムと一致しない
        assert!(!common::verify_checksum(tampered_data, &original_checksum));

        // 正しいデータはチェックサムと一致する
        assert!(common::verify_checksum(original_data, &original_checksum));
    }
}

#[wasm_bindgen(start)]
pub fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App /> });
}
