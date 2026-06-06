use gloo_net::http::Request;
use leptos::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::{Blob, FormData, HtmlInputElement};

const API_BASE: &str = "http://localhost:3000";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub authorized: bool,
    pub user_hash: Option<String>,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentMeta {
    pub cid: String,
    pub checksum: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub status: String,
}

#[component]
pub fn App() -> impl IntoView {
    let (view, set_view) = create_signal("login".to_string());
    let (user_hash, set_user_hash) = create_signal(None::<String>);
    let (token, set_token) = create_signal(None::<String>);

    let handle_login_success = move |hash: String, jwt: String| {
        set_user_hash.set(Some(hash));
        set_token.set(Some(jwt));
        set_view.set("dashboard".to_string());
    };

    view! {
        <div class="min-h-screen bg-black text-white font-mono">
            <nav class="border-b border-white/10 p-4 flex justify-between items-center sticky top-0 bg-black z-50">
                <h1 class="text-xl font-bold tracking-tighter">OPENILLUSTRATION / GUILD (RUST)</h1>
                <Show when=move || view.get() != "login">
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
                        <span class="text-green-500 border border-green-500/20 px-2">
                            {move || user_hash.get().map(|h| h[0..8.min(h.len())].to_string()).unwrap_or_default()}
                        </span>
                    </div>
                </Show>
            </nav>

            <main class="container mx-auto p-4">
                {move || match view.get().as_str() {
                    "login" => view! { <Login on_success=handle_login_success /> }.into_view(),
                    "dashboard" => view! { <Dashboard /> }.into_view(),
                    "upload" => view! { <Upload user_hash=user_hash.get().unwrap_or_default() token=token.get().unwrap_or_default() /> }.into_view(),
                    "moderation" => view! {
                        <Moderation token=token.get().unwrap_or_default() user_hash=user_hash.get().unwrap_or_default() />
                    }.into_view(),
                    _ => view! { <div class="text-center mt-20 text-gray-600">"UNKNOWN_VIEW"</div> }.into_view(),
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
    let (loading, set_loading) = create_signal(false);

    let do_login = move |_| {
        set_loading.set(true);
        let on_success = on_success.clone();
        spawn_local(async move {
            let res = Request::post(&format!("{}/auth/verify", API_BASE))
                .json(&serde_json::json!({
                    "presentation_token": "mock_token",
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
fn Dashboard() -> impl IntoView {
    let (items, set_items) = create_signal(Vec::<ContentMeta>::new());
    let (error, set_error) = create_signal(String::new());

    create_effect(move |_| {
        spawn_local(async move {
            match Request::get(&format!("{}/content", API_BASE)).send().await {
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
                        <ContentViewer cid=item.cid.clone() expected_checksum=item.checksum.clone() />
                    }
                }
            />
            <Show when=move || items.get().is_empty()>
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

#[component]
fn ContentViewer(cid: String, expected_checksum: String) -> impl IntoView {
    let cid_label = cid.clone();
    let (verified, set_verified) = create_signal(None::<bool>);
    let (content_url, set_content_url) = create_signal(String::new());

    create_effect(move |_| {
        let cid = cid.clone();
        let expected_checksum = expected_checksum.clone();
        spawn_local(async move {
            let url = format!("{}/content/{}", API_BASE, cid);
            let res = Request::get(&url).send().await;

            if let Ok(response) = res {
                let header_checksum = response
                    .headers()
                    .get("x-forensic-checksum")
                    .map(|s| s.to_string());
                let bytes = response.binary().await.unwrap_or_default();

                let reference = header_checksum.unwrap_or(expected_checksum);
                let is_ok = common::verify_checksum(&bytes, &reference);

                set_verified.set(Some(is_ok));
                if is_ok && !bytes.is_empty() {
                    let blob = Blob::new_with_u8_array_sequence(&js_sys::Array::of1(
                        &js_sys::Uint8Array::from(&bytes[..]).into(),
                    ))
                    .unwrap();
                    let object_url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();
                    set_content_url.set(object_url);
                }
            } else {
                set_verified.set(Some(false));
            }
        });
    });

    view! {
        <div class="p-4 border border-white/10 bg-black text-white space-y-4">
            <p class="text-[8px] text-gray-600 truncate">{cid_label.clone()}</p>
            <div class="aspect-square bg-white/5 flex items-center justify-center relative overflow-hidden">
                {move || match verified.get() {
                    Some(true) => view! { <img src=content_url.get() class="max-w-full max-h-full object-contain" /> }.into_view(),
                    Some(false) => view! {
                        <div class="text-center p-4">
                            <p class="text-red-500 font-bold text-[10px] mb-2 uppercase">"Integrity Breach Detected"</p>
                            <p class="text-gray-500 text-[8px]">"Wasm checksum does not match forensic registry."</p>
                        </div>
                    }.into_view(),
                    None => view! { <span class="text-[10px] text-gray-600 animate-pulse uppercase tracking-tighter">"Verifying_Integrity..."</span> }.into_view(),
                }}
            </div>
            <div class="text-[10px] tracking-widest font-bold">
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
            </div>
        </div>
    }
}

#[component]
fn Upload(user_hash: String, token: String) -> impl IntoView {
    let (status, set_status) = create_signal(String::new());
    let (loading, set_loading) = create_signal(false);
    let (file, set_file) = create_signal(None::<web_sys::File>);

    let on_file_change = move |ev| {
        let input: HtmlInputElement = event_target(&ev);
        if let Some(files) = input.files()
            && let Some(f) = files.get(0)
        {
            set_file.set(Some(f));
        }
    };

    let sign_and_upload = move |_| {
        let f = file.get().unwrap();
        let user_hash = user_hash.clone();
        let token = token.clone();
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
                <button on:click=sign_and_upload disabled=move || file.get().is_none() || loading.get()
                    class="w-full py-4 bg-white text-black font-bold uppercase tracking-widest hover:bg-gray-200 transition-colors disabled:opacity-50">
                    {move || if loading.get() { "Processing..." } else { "Sign & Submit to Evidence Ledger" }}
                </button>
                <Show when=move || !status.get().is_empty()>
                    <div class="p-4 bg-white/5 border border-white/20 text-xs">
                        <span class="text-gray-500 mr-2">"[STATUS]"</span> {move || status.get()}
                    </div>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn Moderation(token: String, user_hash: String) -> impl IntoView {
    let (cid, set_cid) = create_signal(String::new());
    let (reason, set_reason) = create_signal(String::new());
    let (status, set_status) = create_signal(String::new());

    let submit_report = move |_| {
        let cid = cid.get();
        let reason = reason.get();
        let token = token.clone();
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
            set_status.set(match res {
                Ok(r) if r.ok() => "Report accepted. Content marked Reported.".to_string(),
                Ok(_) => "Report failed.".to_string(),
                Err(_) => "Network error.".to_string(),
            });
        });
    };

    view! {
        <div class="max-w-xl mx-auto mt-12 border border-white/10 p-8 space-y-6">
            <h2 class="text-xl font-bold tracking-widest uppercase">"Jury Moderation Console"</h2>
            <p class="text-[10px] text-gray-500">
                "Reporter: " {user_hash[0..8.min(user_hash.len())].to_string()}
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
            <Show when=move || !status.get().is_empty()>
                <p class="text-xs text-gray-400">{move || status.get()}</p>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wasm_checksum_matches_common_crate() {
        let data = b"guild forensic payload";
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(data);
        let hex = hex::encode(hasher.finalize());
        assert!(common::verify_checksum(data, &hex));
    }
}

#[wasm_bindgen(start)]
pub fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App /> });
}
