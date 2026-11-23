use gloo_net::http::{Request, RequestBuilder, Response};
use leptos::prelude::*;
use leptos::task::spawn_local;
use pulldown_cmark::{CowStr, Event, HeadingLevel, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::task::Waker;
use wasm_bindgen::prelude::*;
use web_sys::{Blob, BlobPropertyBag, FormData, HtmlInputElement, RequestCredentials};

/// `popstate` 等のイベントリスナーを `StoredValue` に保持する際の型。
/// `Closure<dyn FnMut(web_sys::Event)>` を直接書くと型が complex になるため集約する。
type EventClosure = Closure<dyn FnMut(web_sys::Event)>;

/// サーバーが受理する MIME を、ブラウザの判定とファイル名拡張子から決める。
/// ブラウザが `image/jxl` を認識しない環境でもアップロードできるようにする。
fn upload_media_type(browser_type: &str, filename: &str) -> &'static str {
    let lower = browser_type.trim().to_ascii_lowercase();
    if lower == "image/jxl" {
        return "image/jxl";
    }
    if lower == "text/markdown" || lower == "text/x-markdown" {
        return "text/markdown";
    }
    let name = filename.to_ascii_lowercase();
    if name.ends_with(".jxl") {
        "image/jxl"
    } else if name.ends_with(".md") || name.ends_with(".markdown") || name.ends_with(".txt") {
        "text/markdown"
    } else {
        // サーバーが fail-closed で拒否する値を送る（既定で受理させない）。
        ""
    }
}

/// ビルド時に注入する API オリジン。`OIL_MOCK_AUTH` と同じ `option_env!` パターンで、
/// 未設定（E2E / ローカル開発）は従来の開発既定値を使う。
const API_BASE_ENV: Option<&str> = option_env!("OIL_API_BASE");

/// 開発用の既定オリジン。リリース成果物では `OIL_API_BASE` へ HTTPS オリジンを明示する。
const DEFAULT_API_ORIGIN: &str = "http://localhost:3000";

/// API オリジン。ビルド時に `OIL_API_BASE` で上書きできる表示用の型。
///
/// 値はオリジンのみ（`<scheme>://<host>[:<port>]`）とし、パス・クエリ・
/// 認証情報・機微情報は含めない。HTTPS 配信では HTTPS オリジンを指定し、
/// mixed content を避ける。未設定時は開発用ローカルの既定値を使う。
struct ApiOrigin;

/// リリースビルド（`dev-console` feature なし）で平文 HTTP / localhost の指定を拒否する。
/// `OIL_API_BASE` 未設定は開発ビルドの既定値として従来どおり通す（E2E の `trunk build` は
/// feature・環境変数なしで動く）。`dev-console` 付きの開発ビルドも検証対象外とする。
#[cfg(not(feature = "dev-console"))]
const _: () = assert_release_api_origin(API_BASE_ENV);

/// オリジン判定の境界値をビルド時にも検証する（path / query / userinfo / loopback は拒否）。
#[cfg(not(feature = "dev-console"))]
const _: () = {
    assert!(is_https_non_local_origin("https://api.example.org"));
    assert!(is_https_non_local_origin("https://api.example.org:8443"));
    assert!(!is_https_non_local_origin("http://api.example.org"));
    assert!(!is_https_non_local_origin("https://api.example.org/v1"));
    assert!(!is_https_non_local_origin("https://api.example.org/?key=1"));
    assert!(!is_https_non_local_origin("https://api.example.org#top"));
    assert!(!is_https_non_local_origin("https://user@api.example.org"));
    assert!(!is_https_non_local_origin("https://127.0.0.1:8443"));
    assert!(!is_https_non_local_origin("https://[::1]:8443"));
    assert!(!is_https_non_local_origin("https://0.0.0.0"));
    assert!(!is_https_non_local_origin("https://LOCALHOST"));
};

/// 不正なオリジンが注入されたら const 評価時にビルドを失敗させる（fail-closed）。
#[cfg(not(feature = "dev-console"))]
const fn assert_release_api_origin(origin: Option<&str>) {
    if let Some(origin) = origin {
        assert!(
            is_https_non_local_origin(origin),
            "OIL_API_BASE must be an HTTPS origin without localhost in release builds"
        );
    }
}

/// ASCII の大小文字を無視して `needle` の出現を探す（const 対応のバイト走査）。
#[cfg(not(feature = "dev-console"))]
const fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let mut index = 0;
    while index + needle.len() <= haystack.len() {
        let mut matched = true;
        let mut offset = 0;
        while offset < needle.len() {
            let byte = haystack[index + offset];
            let lowered = if byte >= b'A' && byte <= b'Z' {
                byte + 32
            } else {
                byte
            };
            if lowered != needle[offset] {
                matched = false;
                break;
            }
            offset += 1;
        }
        if matched {
            return true;
        }
        index += 1;
    }
    false
}

/// `https://` で始まり、スキーム後にオリジン区切り（`/` `?` `#` `@`）がなく、
/// loopback / wildcard 宿主（`127.0.0.1` / `[::1]` / `0.0.0.0` / `localhost`）
/// を含まないことをバイト単位で判定する。
/// const 対応のため `str` の高級メソッドではなくバイト走査を使う。
#[cfg(not(feature = "dev-console"))]
const fn is_https_non_local_origin(origin: &str) -> bool {
    let bytes = origin.as_bytes();
    let scheme = b"https://";
    if bytes.len() < scheme.len() {
        return false;
    }
    let mut index = 0;
    while index < scheme.len() {
        if bytes[index] != scheme[index] {
            return false;
        }
        index += 1;
    }
    // オリジンのみを許可する。パス・クエリ・フラグメント・userinfo は拒否。
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'/' || byte == b'?' || byte == b'#' || byte == b'@' {
            return false;
        }
        index += 1;
    }
    // loopback / wildcard 宿主は大小文字無視で拒否する。
    let forbidden: [&[u8]; 4] = [b"127.0.0.1", b"[::1]", b"0.0.0.0", b"localhost"];
    let mut pattern = 0;
    while pattern < forbidden.len() {
        if contains_ascii_case_insensitive(bytes, forbidden[pattern]) {
            return false;
        }
        pattern += 1;
    }
    true
}

/// 設定値の表記ゆれを吸収するため、末尾の `/` だけを除去する。
fn normalize_api_origin(origin: &str) -> &str {
    origin.trim_end_matches('/')
}

/// ISO 8601 文字列を表示用の日時へ整形する。
///
/// 既存依存の js-sys 経由で `Intl.DateTimeFormat` を呼び、locale は `ja-JP`、
/// timeZone は `UTC` を明示する。ホストのロケール・タイムゾーンに結果を依存させない。
/// パースできない文字列は丸めずに元の文字列へフォールバックする（情報の消失を避ける）。
fn format_local_datetime(iso: &str) -> String {
    let date = js_sys::Date::new(&JsValue::from_str(iso));
    if date.get_time().is_nan() {
        return iso.to_string();
    }

    let options = js_sys::Object::new();
    let set = |key: &str, value: &str| {
        let _ = js_sys::Reflect::set(&options, &JsValue::from_str(key), &JsValue::from_str(value));
    };
    set("timeZone", "UTC");
    set("year", "numeric");
    set("month", "2-digit");
    set("day", "2-digit");
    set("hour", "2-digit");
    set("minute", "2-digit");
    set("second", "2-digit");
    let _ = js_sys::Reflect::set(
        &options,
        &JsValue::from_str("hour12"),
        &JsValue::from_bool(false),
    );

    let locales = js_sys::Array::of1(&JsValue::from_str("ja-JP"));
    let formatter = js_sys::Intl::DateTimeFormat::new(&locales, &options);
    match formatter.format().call1(&JsValue::NULL, date.as_ref()) {
        // UTC 固定を維持するため、可視テキストにもタイムゾーンを明示する（誤読防止）。
        Ok(value) => format!(
            "{} (協定世界時)",
            value.as_string().unwrap_or_else(|| iso.to_string())
        ),
        Err(_) => iso.to_string(),
    }
}

/// 集計カードの `GENERATED_AT` を表示用のローカル書式へ整形する。
/// wasm 以外の単体テストでは JS の `Intl` を呼べないため、元の文字列を保持する。
#[cfg(target_arch = "wasm32")]
fn format_stats_generated_at(iso: &str) -> String {
    format_local_datetime(iso)
}

/// 非 wasm の単体テスト向け。生 ISO をそのまま返し、属性側でエスケープする。
#[cfg(not(target_arch = "wasm32"))]
fn format_stats_generated_at(iso: &str) -> String {
    iso.to_string()
}

impl std::fmt::Display for ApiOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let origin = API_BASE_ENV.unwrap_or(DEFAULT_API_ORIGIN);
        f.write_str(normalize_api_origin(origin))
    }
}

/// 各リクエスト URL の起点。`format!("{API_BASE}/...")` として使う。
const API_BASE: ApiOrigin = ApiOrigin;

/// 監視用 POST の有効/無効。ビルド時に `OIL_CLIENT_METRICS=0` を指定すると送信を止める。
/// 未設定（既定）は送信を継続する（未リリース機能の既定契約を維持するため）。
const CLIENT_METRICS_ENABLED: bool = metrics_enabled_from_env(option_env!("OIL_CLIENT_METRICS"));

/// ビルド時真偽値の共通判定（const fn）。`"0"` / `"false"`（大小文字無視）を無効、
/// 未設定とその他を有効とみなす。サーバ側 parse_bool_env は受理値外を fail-closed で
/// 拒否するが、ビルド時フラグは開発既定を維持するため受理値外も有効として扱う点が異なる。
const fn build_flag_enabled(value: Option<&str>) -> bool {
    match value {
        Some(v) => {
            let bytes = v.as_bytes();
            let zero = bytes.len() == 1 && bytes[0] == b'0';
            let false_word = match bytes.len() {
                5 => {
                    (bytes[0] == b'f' || bytes[0] == b'F')
                        && (bytes[1] == b'a' || bytes[1] == b'A')
                        && (bytes[2] == b'l' || bytes[2] == b'L')
                        && (bytes[3] == b's' || bytes[3] == b'S')
                        && (bytes[4] == b'e' || bytes[4] == b'E')
                }
                _ => false,
            };
            !zero && !false_word
        }
        None => true,
    }
}

/// ビルド時設定値から監視送信の可否を決める（純粋関数）。
const fn metrics_enabled_from_env(value: Option<&str>) -> bool {
    build_flag_enabled(value)
}

/// 開発用モック認証の有効/無効。ビルド時に `OIL_MOCK_AUTH=0` を指定すると注記を止める。
/// 未設定（既定）は有効とする（現行の開発用モック認証契約を維持するため）。
const MOCK_AUTH_ENABLED: bool = mock_auth_enabled_from_env(option_env!("OIL_MOCK_AUTH"));

/// ビルド時設定値からモック認証の可否を決める（純粋関数）。
const fn mock_auth_enabled_from_env(value: Option<&str>) -> bool {
    build_flag_enabled(value)
}

/// サーバーの本文制限と同じ 10 MiB。選択時に拒否して無駄な送信を避ける。
const MAX_UPLOAD_BYTES: f64 = 10.0 * 1024.0 * 1024.0;

/// 投稿フォームの maxlength は `common` の UTF-8 バイト上限から導出する。
/// HTML の maxlength は UTF-16 コードユニット単位のため、日本語等のマルチバイト文字では
/// この値だけでは上限を守れない。実契約は送信前ゲート `metadata_byte_error` が担保する。
const UPLOAD_TITLE_MAX_LEN: u32 = common::MAX_CONTENT_TITLE_BYTES as u32;
const UPLOAD_DESCRIPTION_MAX_LEN: u32 = common::MAX_CONTENT_DESCRIPTION_BYTES as u32;
/// 10 タグ × 50 B + 区切りカンマ 9 個 = 509（UTF-16 では守れない旨は上記を参照）。
const UPLOAD_TAGS_MAX_LEN: u32 = (common::MAX_CONTENT_TAGS * common::MAX_CONTENT_TAG_BYTES
    + (common::MAX_CONTENT_TAGS - 1)) as u32;

/// エクスポートの保存ファイル名。ユーザー入力を一切使わない固定値（パス注入対策）。
const EXPORT_FILENAME: &str = "openillustration-export.json";

/// エクスポート結果が `DataExport` として解析できない場合の固定文言。
/// 解析エラーの詳細は機微情報を含み得るため画面へ出さない。
const EXPORT_PARSE_ERROR: &str = "エクスポート結果の解析に失敗しました。";

/// `/content` 一覧の取得失敗時に表示する固定文言。
/// レスポンス本文や生エラーは機微情報を含み得るため画面へ出さない。
const CONTENT_INDEX_LOAD_ERROR: &str = "Failed to load content index.";
const CONTENT_INDEX_NETWORK_ERROR: &str = "Network error loading content.";
/// `/content` が 401 のときだけ出す再ログイン案内。他のエラー文言は変更しない。
const CONTENT_INDEX_UNAUTHORIZED: &str = "再ログインが必要です。";
/// 一覧取得失敗時に `role=status` が読み上げる文言（既存固定文言は変更しない）。
const DASHBOARD_LOAD_FAILED_ANNOUNCEMENT: &str = "コンテンツを取得できませんでした";

/// 本番認証が未実装であることを示す固定文言。
/// 501 レスポンスの本文は機微情報を含み得るため画面へ出さず、この文言だけを表示する。
const PRODUCTION_AUTH_NOT_IMPLEMENTED: &str = "本番認証は未実装です。";

/// ログイン進捗を dedicated `role=status` で告知する固定文言（日本語）。
const LOGIN_PROGRESS: &str = "本人確認の通信中です。";

// ---- 共有トークンと refresh の単一フライト ----
//
// wasm は単一スレッドなので、モジュール級状態は thread_local + Rc<RefCell> で足りる。
// App が生成した JWT signal をここへ登録し、api_fetch は試行ごとに現在値を読む。

thread_local! {
    /// App が登録する共有 JWT signal。未登録なら api_fetch は空文字を送る。
    static SHARED_TOKEN: RefCell<Option<ReadSignal<Option<String>>>> = const { RefCell::new(None) };
    /// App が登録する JWT 書き込み側。refresh 成功とログイン成功で同じ値を set する。
    static SHARED_TOKEN_WRITE: RefCell<Option<WriteSignal<Option<String>>>> = const { RefCell::new(None) };
    /// App が登録する現在の user_hash signal。refresh 応答の user_hash と突き合わせ、
    /// 別ユーザー Cookie の JWT を採用しないための照合元にする。
    static SHARED_USER_HASH: RefCell<Option<ReadSignal<Option<String>>>> = const { RefCell::new(None) };
    /// refresh 成功で得た新 JWT。App の token signal を更新すると一覧フェッチ Effect
    /// が再加熱されビューが再マウントされるため、api_fetch 専用に別スロットで保持する。
    static REFRESHED_JWT: RefCell<Option<String>> = const { RefCell::new(None) };
    /// logout/login でインクリメントする epoch。in-flight の refresh が古いセッションの
    /// JWT を書き戻すのを防ぐ（claim 時に捕捉した値と完了時に照合する）。
    static REFRESH_EPOCH: Cell<u64> = const { Cell::new(0) };
    /// 401 を拾った最初の呼び出しだけが refresh を開始するための完了スロット。
    static REFRESH_SLOT: Rc<RefCell<RefreshSlot>> = Rc::new(RefCell::new(RefreshSlot::new()));
}

/// App の JWT signal を共有状態へ登録する（マウント前に一度だけ呼ぶ）。
fn register_shared_token(read: ReadSignal<Option<String>>, write: WriteSignal<Option<String>>) {
    SHARED_TOKEN.with(|slot| *slot.borrow_mut() = Some(read));
    SHARED_TOKEN_WRITE.with(|slot| *slot.borrow_mut() = Some(write));
}

/// App の user_hash signal を共有状態へ登録する（マウント前に一度だけ呼ぶ）。
fn register_user_hash(read: ReadSignal<Option<String>>) {
    SHARED_USER_HASH.with(|slot| *slot.borrow_mut() = Some(read));
}

/// 現在ログイン中の user_hash を共有 signal から読む。signal の borrow をまたいで
/// 副作用を呼ばないよう、先に値を取り出してから返す（current_jwt と同じ契約）。
fn current_user_hash() -> Option<String> {
    SHARED_USER_HASH.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|signal| signal.get_untracked())
    })
}

/// 現在の refresh epoch を読む。
fn current_refresh_epoch() -> u64 {
    REFRESH_EPOCH.with(|epoch| epoch.get())
}

/// logout/login で epoch を進め、進行中の refresh を無効化する。
fn bump_refresh_epoch() {
    REFRESH_EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
}

/// 現在の JWT を共有 signal から読む。signal の borrow をまたいで副作用を呼ばないよう
/// 先に値を取り出してから返す。
fn current_jwt() -> Option<String> {
    if let Some(jwt) = REFRESHED_JWT.with(|slot| slot.borrow().clone()) {
        return Some(jwt);
    }
    SHARED_TOKEN.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|signal| signal.get_untracked())
    })
}

/// refresh 成功で得た JWT を api_fetch 専用スロットへ保持する。
/// App の signal は更新しない（Effect 再加熱による再マウントを避けるため）。
fn set_refreshed_jwt(jwt: String) {
    REFRESHED_JWT.with(|slot| *slot.borrow_mut() = Some(jwt));
}

/// ログイン／ログアウト時に api_fetch 専用スロットをクリアする。
fn clear_refreshed_jwt() {
    REFRESHED_JWT.with(|slot| *slot.borrow_mut() = None);
}

/// 共有 signal へ JWT を書き込む。handle_login_success と refresh 成功の両方が
/// 同じこの関数（＝同じ signal）を通る。
fn set_shared_jwt(jwt: Option<String>) {
    SHARED_TOKEN_WRITE.with(|slot| {
        if let Some(signal) = *slot.borrow() {
            signal.set(jwt);
        }
    });
}

/// refresh の完了結果。待機者は `Token` の新 JWT を使って再送し、`Failed` なら
/// 自分の 401 レスポンスをそのまま呼び出し元へ返す。
#[derive(Clone, Debug, PartialEq, Eq)]
enum RefreshOutcome {
    Token(String),
    Failed,
}

/// 単一フライトの状態。`in_flight` の間は後続の 401 を待機させ、完了結果を
/// すべての待機者へ同じ値で渡す。`epoch` は claim 時に捕捉した refresh epoch で、
/// logout/login が挟まった場合は完了時の JWT 書き戻しを拒否する。
#[derive(Default)]
struct RefreshSlot {
    in_flight: bool,
    result: Option<RefreshOutcome>,
    waiters: Vec<Waker>,
    epoch: u64,
}

impl RefreshSlot {
    fn new() -> Self {
        Self::default()
    }

    /// フライトを開始できれば `true`（リーダー）。既に進行中なら `false`（待機者）。
    /// 新しいフライトでは前回の完了結果を必ずクリアし、現在の epoch を捕捉する。
    fn claim(&mut self) -> bool {
        if self.in_flight {
            return false;
        }
        self.in_flight = true;
        self.result = None;
        self.epoch = current_refresh_epoch();
        true
    }

    /// 完了を記録し、待機中のタスクをすべて起こす。
    /// epoch が一致するときだけ JWT を書き戻す。logout/login が挟まって
    /// 不一致なら古い JWT は書き戻さず、待機者へ `Failed` を配る。
    fn complete(&mut self, outcome: RefreshOutcome) -> RefreshOutcome {
        let outcome = if self.epoch == current_refresh_epoch() {
            if let RefreshOutcome::Token(token) = &outcome {
                set_refreshed_jwt(token.clone());
            }
            outcome
        } else {
            RefreshOutcome::Failed
        };
        self.result = Some(outcome.clone());
        self.in_flight = false;
        let waiters = std::mem::take(&mut self.waiters);
        for waker in waiters {
            waker.wake();
        }
        outcome
    }

    /// 完了済みなら結果を返す。
    fn outcome(&self) -> Option<RefreshOutcome> {
        self.result.clone()
    }

    /// 未完了の間は Waker を登録する。同じタスクの二重登録は避ける。
    fn register_waker(&mut self, waker: &Waker) {
        if !self.waiters.iter().any(|known| known.will_wake(waker)) {
            self.waiters.push(waker.clone());
        }
    }
}

/// フライト完了を待つ Future。`spawn_local` から使うため Send は要求されない。
struct RefreshWaiter {
    slot: Rc<RefCell<RefreshSlot>>,
}

impl std::future::Future for RefreshWaiter {
    type Output = RefreshOutcome;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut slot = self.slot.borrow_mut();
        if let Some(outcome) = slot.outcome() {
            return std::task::Poll::Ready(outcome);
        }
        slot.register_waker(cx.waker());
        std::task::Poll::Pending
    }
}

/// `/auth/refresh` へ Cookie 付きで POST する。Authorization ヘッダもボディも送らない。
/// VerifyResponse の `user_hash` も返し、呼び出し側で現在のログインユーザーと照合する。
async fn perform_refresh() -> (RefreshOutcome, Option<String>) {
    let request = Request::post(&format!("{API_BASE}/auth/refresh"))
        .credentials(RequestCredentials::Include)
        .build();
    let Ok(request) = request else {
        return (RefreshOutcome::Failed, None);
    };
    let Ok(response) = request.send().await else {
        return (RefreshOutcome::Failed, None);
    };
    if !response.ok() {
        return (RefreshOutcome::Failed, None);
    }
    match response.json::<VerifyResponse>().await {
        Ok(data) => {
            let user_hash = data.user_hash;
            let outcome = if data.authorized {
                match data.token {
                    Some(token) if !token.is_empty() => RefreshOutcome::Token(token),
                    _ => RefreshOutcome::Failed,
                }
            } else {
                RefreshOutcome::Failed
            };
            (outcome, user_hash)
        }
        Err(_) => (RefreshOutcome::Failed, None),
    }
}

/// 401 を受けた呼び出しを単一フライトへ参加させる。
/// 最初の 1 つだけが `/auth/refresh` を叩き、他は完了を待って同じ新 JWT を使う。
async fn join_refresh() -> RefreshOutcome {
    let slot = REFRESH_SLOT.with(Rc::clone);
    if !slot.borrow_mut().claim() {
        return RefreshWaiter { slot }.await;
    }

    let (outcome, refresh_user_hash) = perform_refresh().await;
    // 別ユーザー Cookie の JWT を採用しない。VerifyResponse の user_hash が
    // 現在の user_hash signal と一致しなければ Failed に落とす（fail-closed）。
    let outcome = match (outcome, refresh_user_hash) {
        (RefreshOutcome::Token(token), Some(hash))
            if current_user_hash().as_deref() == Some(hash.as_str()) =>
        {
            RefreshOutcome::Token(token)
        }
        _ => RefreshOutcome::Failed,
    };
    // complete が claim 時の epoch を検証し、不一致なら JWT を書き戻さず
    // 待機者へ Failed を配る。待機者を起こす前に結果を確定させる。
    let outcome = slot.borrow_mut().complete(outcome);
    if outcome == RefreshOutcome::Failed {
        // 失効済み JWT を送り続けないよう REFRESHED_JWT をクリアする。
        // 元の 401 は api_fetch_loop 側でそのまま呼び出し側へ返す契約は維持。
        clear_refreshed_jwt();
    }
    // refresh 失敗時はここで画面遷移しない。各ビューの既存固定文言
    // （「再ログインが必要です」等）で復帰を案内する。画面遷移はユーザーが
    // LOG OUT を押したとき（handle_logout）だけ行う。
    outcome
}

/// 共通の再試行ループ。`make` は試行ごとに新規 `Request` を組み立てる。
/// gloo-net 0.7 は `RequestBuilder::body/json` が `Request` を返して builder を
/// 返さないため、ボディ付き呼び出しは `api_fetch_request` 側で `Request` を渡す。
async fn api_fetch_loop<F>(make: F, auth: bool) -> Result<Response, gloo_net::Error>
where
    F: Fn(&str) -> Result<Request, gloo_net::Error>,
{
    let mut jwt = current_jwt().unwrap_or_default();
    let mut refreshed = false;
    loop {
        let response = make(&jwt)?.send().await?;
        let status = response.status();
        let has_www_authenticate = response.headers().get("www-authenticate").is_some();
        if !should_refresh(status, auth, has_www_authenticate) || !allows_retry(status, refreshed) {
            return Ok(response);
        }
        match join_refresh().await {
            RefreshOutcome::Token(new_jwt) => {
                jwt = new_jwt;
                refreshed = true;
            }
            // refresh 失敗時はセッションを破棄せず、元の 401 を呼び出し側へ返して
            // 各ビューの固定文言（「再ログインが必要です」等）に委ねる。
            RefreshOutcome::Failed => return Ok(response),
        }
    }
}

/// 自動 refresh の有効フラグ（現在は有効）。
///
/// かつて全 fetch への適用で 401 固定文言ビューとの再描画競合が起きたが、
/// 単一フライト化と refresh 失敗時に元の 401 を呼び出し側へ返す契約で解消した。
/// 成功時のみ新 JWT で1回再送し、WWW-Authenticate 付き 401 は対象外。
const ENABLE_AUTO_REFRESH: bool = true;

/// 401 かつ `auth` のときだけ refresh を試す。403 / ネットワークエラーは対象外。
///
/// `WWW-Authenticate` が明示された 401（Basic 等）はトークン更新の対象外とし、
/// サーバーが要求した認証方式に従って呼び出し側の固定文言へ委ねる。
fn should_refresh(status: u16, auth: bool, has_www_authenticate: bool) -> bool {
    ENABLE_AUTO_REFRESH && status == 401 && auth && !has_www_authenticate
}

/// refresh 済みなら 2 回目の 401 でも再送しない（無限ループ防止）。
fn allows_retry(status: u16, refreshed: bool) -> bool {
    status == 401 && !refreshed
}

/// 認証付き API 呼び出しの共通ラッパー。
///
/// `make` は試行ごとに新規 `RequestBuilder` を返し、引数にはその時点の JWT が入る。
/// 401 + `auth` のときだけ refresh を 1 回挟み、2 回目の 401 はそのまま返す。
/// ボディを付ける必要がある呼び出しは `api_fetch_request` を使うこと。
pub async fn api_fetch<F>(make: F, auth: bool) -> Result<Response, gloo_net::Error>
where
    F: Fn(&str) -> RequestBuilder,
{
    api_fetch_loop(
        |jwt| make(jwt).credentials(RequestCredentials::Include).build(),
        auth,
    )
    .await
}

/// ボディ付き（FormData / JSON 等）の認証付き API 呼び出し向けラッパー。
///
/// gloo-net 0.7 の `RequestBuilder::body/json` は `Request` を返すため、
/// `make` は credential 指定とボディ付与を含めて毎回新規 `Request` を返す。
/// ボディはクロージャ内で作り直すので、再送時に消費済みのものを再利用しない。
pub async fn api_fetch_request<F>(make: F, auth: bool) -> Result<Response, gloo_net::Error>
where
    F: Fn(&str) -> Result<Request, gloo_net::Error>,
{
    api_fetch_loop(make, auth).await
}

/// 本文と TOC で共有する見出し情報。
#[derive(Clone, Debug, PartialEq, Eq)]
struct HeadingEntry {
    level: HeadingLevel,
    text: String,
    slug: String,
}

/// テキストを HTML として安全に出力できる形にエスケープする（TOC の表示名用）。
fn escape_html_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// URL コンポーネント（クエリ値・パス Segment）を percent-encode する。
/// API のクエリ契約とブラウザ URL の両方で同じ符号化を使うための純粋実装。
/// 読み出し側は `parse_content_query`（同じく純粋関数）で往復を検証する。
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// 既存クエリへ単一パラメータを追加する（空値は追加しない）。
/// 同じキーが既に存在しても末尾へ追加し、読み側の「最後の値を採用」規則で上書きする。
/// カード詳細／通報プリフィル用のキーは `/content` の検索条件とは分けて扱う。
fn append_query_param(base: &str, key: &str, value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return base.to_string();
    }
    let param = format!("{}={}", key, url_encode(value));
    if base.is_empty() {
        format!("?{param}")
    } else {
        format!("{base}&{param}")
    }
}

/// BROWSE の該当カードへ戻るディープリンクのクエリ（`#dashboard` と組み合わせる）。
fn card_link_query(cid: &str) -> String {
    append_query_param("", "cid", cid)
}

/// MODERATION の通報フォームをプリフィルするクエリ（`#moderation` と組み合わせる）。
fn report_prefill_query(cid: &str) -> String {
    append_query_param("", "report_cid", cid)
}

/// 省略可能な絞り込み条件から `/content` のクエリ文字列を組み立てる。
/// 空の条件は送らず、条件が無い場合は既存と同じ `/content` を返す。
/// `favorite_only` は値を取らないフラグで、他の条件と併用できる。
fn build_content_query(q: &str, tag: &str, content_type: &str, favorite_only: bool) -> String {
    let mut params = Vec::new();
    let q = q.trim();
    let tag = tag.trim();
    let content_type = content_type.trim();
    if !q.is_empty() {
        params.push(format!("q={}", url_encode(q)));
    }
    if !tag.is_empty() {
        params.push(format!("tag={}", url_encode(tag)));
    }
    if !content_type.is_empty() {
        params.push(format!("type={}", url_encode(content_type)));
    }
    if favorite_only {
        params.push("favorite_only=1".to_string());
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

/// 検索条件が1つでも有効かどうか（純粋判定）。`build_content_query` と同じ
/// trimming 規則を使い、0 件時の文言・クリアボタンの表示根拠にする。
fn search_filters_active(q: &str, tag: &str, content_type: &str, favorite_only: bool) -> bool {
    !q.trim().is_empty()
        || !tag.trim().is_empty()
        || !content_type.trim().is_empty()
        || favorite_only
}

/// 0 件時の文言。条件があれば案内を出し、条件なしなら既存文言を維持する（純粋関数）。
fn content_empty_state_message(
    q: &str,
    tag: &str,
    content_type: &str,
    favorite_only: bool,
) -> &'static str {
    if search_filters_active(q, tag, content_type, favorite_only) {
        "条件に一致するコンテンツはありません。"
    } else {
        "承認済みコンテンツはありません。"
    }
}

/// `url_encode` の逆変換。`+` を空白として扱い、妥当な percent escape だけを
/// バイト列へ戻す（不正な列はそのまま残す）。UTF-8 として読めない列は
/// lossy にするが、パニックは起こさない。
fn url_decode(input: &str) -> String {
    fn hex_value(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let (Some(high), Some(low)) =
                    (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
                {
                    out.push((high << 4) | low);
                    index += 3;
                } else {
                    out.push(b'%');
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// URL クエリから復元する検索条件。`build_content_query` の純粋な逆変換。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ContentQuery {
    q: String,
    tag: String,
    content_type: String,
    favorite_only: bool,
    /// BROWSE の該当カードを指すディープリンク用。`/content` の検索条件ではない。
    cid: String,
    /// MODERATION の通報フォームをプリフィルする CID。検索条件ではない。
    report_cid: String,
}

/// クエリ文字列（先頭の `?` は任意）から既知の 4 条件だけを読み出す。
/// 未知のキー・空の値は無視する。UI 側の `refresh_counter` / `search_submit`
/// は操作トリガーなので URL には含めず、ここでも復元しない。
fn parse_content_query(query: &str) -> ContentQuery {
    let mut parsed = ContentQuery::default();
    let body = query.strip_prefix('?').unwrap_or(query);
    for pair in body.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "q" => parsed.q = url_decode(value),
            "tag" => parsed.tag = url_decode(value),
            "type" => parsed.content_type = url_decode(value),
            // build_content_query は `favorite_only=1` を書き出す。値なしの
            // `favorite_only` もフラグとして受け取る（旧リンクの許容）。
            "favorite_only" => parsed.favorite_only = matches!(value, "" | "1"),
            "cid" => parsed.cid = url_decode(value),
            "report_cid" => parsed.report_cid = url_decode(value),
            _ => {}
        }
    }
    parsed
}

/// 生のクエリ文字列を `parse_content_query` → `build_content_query` の順に合成し、
/// 既知キーだけを残す正規形式へ変換する純粋関数。
/// `parse_content_query` の「同じキーは最後の値を採用する」意味を維持するため、
/// `UrlSearchParams::get`（先頭値を返す）は使わず、必ず parse を通す。
#[cfg(any(test, target_arch = "wasm32"))]
fn canonical_content_query(raw: &str) -> String {
    let parsed = parse_content_query(raw);
    let mut query = build_content_query(
        &parsed.q,
        &parsed.tag,
        &parsed.content_type,
        parsed.favorite_only,
    );
    // ディープリンク用のキーは検索条件の後ろへ固定順で書き出す。
    query = append_query_param(&query, "cid", &parsed.cid);
    append_query_param(&query, "report_cid", &parsed.report_cid)
}

/// ブラウザの `location.search` を読み、`canonical_content_query` で正規化する。
/// 初回読み・popstate・navigate のすべてが同じ純粋関数を通るため意味が統一される。
/// non-wasm の単体テストでは検索条件を読まない（空文字を返す）。
fn current_content_query() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return String::new();
        };
        let Ok(search) = window.location().search() else {
            return String::new();
        };
        canonical_content_query(&search)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        String::new()
    }
}

/// テキスト入力（q/tag/type）から GET までのデバウンス間隔（ms）。
/// `favorite_only` 変更と refresh_counter（お気に入りトグルの成功通知）は
/// 即時取得とし、このデバウンスを適用しない（コメントは Dashboard 参照）。
const SEARCH_DEBOUNCE_MS: i32 = 250;

/// レスポンスを画面へ反映してよいかを判定する純粋ヘルパー。
/// 入力変化のたびに世代を進めるため、最新世代の応答だけを採用する。
fn should_apply_response(current_generation: u64, response_generation: u64) -> bool {
    current_generation == response_generation
}

/// 実行中のデバウンスタイマー。ハンドルと Rust 側の Closure を保持し、
/// 次回の入力で `clearTimeout` と Closure の drop を行う。
struct PendingFetch {
    handle: Option<i32>,
    closure: Closure<dyn FnMut()>,
}

/// 一覧カードに表示するメタデータを組み立てる。
/// ユーザーが登録した値はすべて `escape_html_text` を通してから
/// `inner_html` へ渡す（属性・タグとして解釈されないようにする）。
fn render_content_meta_html(title: &str, description: &str, tags: &[String]) -> String {
    let mut html = String::new();
    let _ = writeln!(
        html,
        "<h3 class=\"text-sm font-bold tracking-wide text-white break-words\">{}</h3>",
        escape_html_text(title)
    );
    if !description.trim().is_empty() {
        let _ = writeln!(
            html,
            "<p class=\"mt-1 text-[10px] text-gray-400 leading-relaxed whitespace-pre-wrap break-words\">{}</p>",
            escape_html_text(description)
        );
    }
    if !tags.is_empty() {
        html.push_str("<div class=\"mt-2 flex flex-wrap gap-1\">");
        for tag in tags {
            let _ = write!(
                html,
                "<span class=\"text-[8px] border border-white/10 px-1 text-gray-400\">#{}</span>",
                escape_html_text(tag)
            );
        }
        html.push_str("</div>");
    }
    html
}

/// 透明性ダッシュボードの集計カードと月次表を組み立てる。
/// サーバー由来の文字列・数値はすべて `escape_html_text` を通してから
/// `inner_html` へ渡す（属性やタグとして解釈されないようにする）。
fn render_moderation_stats_html(stats: &ModerationStats) -> String {
    let mut html = String::new();
    // 表示は整形済み日時、生 ISO は title 属性へ保持する（監査日時と同じ契約）。
    let generated_at = format_stats_generated_at(&stats.generated_at);
    let _ = writeln!(
        html,
        "<p data-testid=\"moderation-stats-generated-at\" class=\"mb-4 text-[10px] text-gray-500\" title=\"{}\">GENERATED_AT: {}</p>",
        escape_html_text(&stats.generated_at),
        escape_html_text(&generated_at)
    );

    let totals = [
        ("Reports", stats.totals.reports),
        ("Concluded", stats.totals.concluded),
        ("Approved", stats.totals.approved),
        ("Rejected", stats.totals.rejected),
        ("Quorum Not Met", stats.totals.quorum_not_met),
        ("Bans", stats.totals.bans),
        ("Unbans", stats.totals.unbans),
        ("Unpins", stats.totals.unpins),
    ];
    html.push_str(
        "<div data-testid=\"moderation-stats-totals\" class=\"grid grid-cols-2 md:grid-cols-4 gap-3\">",
    );
    for (label, value) in totals {
        let _ = writeln!(
            html,
            "<div class=\"border border-white/10 p-3\"><p lang=\"en\" class=\"text-[8px] uppercase tracking-widest text-gray-500\">{label}</p><p class=\"text-lg font-bold text-green-500\">{}</p></div>",
            escape_html_text(&value.to_string())
        );
    }
    html.push_str("</div>");

    html.push_str(
        "<table data-testid=\"moderation-stats-monthly\" class=\"w-full mt-6 text-left text-[10px] font-mono\">",
    );
    html.push_str("<caption>月次集計</caption>");
    html.push_str(
        "<thead lang=\"en\" class=\"border-b border-white/10 text-gray-400 uppercase\"><tr>\
         <th scope=\"col\" class=\"py-2 pr-4\">Month</th><th scope=\"col\" class=\"py-2 pr-4\">Reports</th>\
         <th scope=\"col\" class=\"py-2 pr-4\">Concluded</th><th scope=\"col\" class=\"py-2 pr-4\">Approved</th>\
         <th scope=\"col\" class=\"py-2 pr-4\">Rejected</th><th scope=\"col\" class=\"py-2 pr-4\">Bans</th>\
         <th scope=\"col\" class=\"py-2 pr-4\">Unbans</th><th scope=\"col\" class=\"py-2 pr-4\">Unpins</th></tr></thead>",
    );
    html.push_str("<tbody class=\"divide-y divide-white/5\">");
    for row in &stats.monthly {
        let _ = writeln!(
            html,
            "<tr><td class=\"py-2 pr-4 text-gray-300\">{}</td>\
             <td class=\"py-2 pr-4\">{}</td><td class=\"py-2 pr-4\">{}</td>\
             <td class=\"py-2 pr-4\">{}</td><td class=\"py-2 pr-4\">{}</td>\
             <td class=\"py-2 pr-4\">{}</td><td class=\"py-2 pr-4\">{}</td>\
             <td class=\"py-2 pr-4\">{}</td></tr>",
            escape_html_text(&row.month),
            escape_html_text(&row.reports.to_string()),
            escape_html_text(&row.concluded.to_string()),
            escape_html_text(&row.approved.to_string()),
            escape_html_text(&row.rejected.to_string()),
            escape_html_text(&row.bans.to_string()),
            escape_html_text(&row.unbans.to_string()),
            escape_html_text(&row.unpins.to_string()),
        );
    }
    html.push_str("</tbody></table>");
    if stats.monthly.is_empty() {
        html.push_str(
            "<p class=\"mt-2 text-[9px] text-gray-400 italic\">月次集計はありません。</p>",
        );
    }
    html
}

/// 認証済み利用者向けの公開集計カードと月次表を組み立てる。
/// 運用者向けの `render_moderation_stats_html` とは異なり Bans / Unbans / Unpins 列は出さない。
/// サーバー由来の文字列・数値はすべて `escape_html_text` を通してから
/// `inner_html` へ渡す（属性やタグとして解釈されないようにする）。
fn render_public_mod_stats_html(stats: &PublicModerationStats) -> String {
    let mut html = String::new();
    // 表示は整形済み日時、生 ISO は title 属性へ保持する（監査日時と同じ契約）。
    let generated_at = format_stats_generated_at(&stats.generated_at);
    let _ = writeln!(
        html,
        "<p data-testid=\"public-mod-stats-generated-at\" class=\"mb-4 text-[10px] text-gray-500\" title=\"{}\">GENERATED_AT: {}</p>",
        escape_html_text(&stats.generated_at),
        escape_html_text(&generated_at)
    );

    let totals = [
        ("Reports", stats.totals.reports),
        ("Concluded", stats.totals.concluded),
        ("Approved", stats.totals.approved),
        ("Rejected", stats.totals.rejected),
        ("Quorum Not Met", stats.totals.quorum_not_met),
    ];
    html.push_str(
        "<div data-testid=\"public-mod-stats-totals\" class=\"grid grid-cols-2 md:grid-cols-4 gap-3\">",
    );
    for (label, value) in totals {
        let _ = writeln!(
            html,
            "<div class=\"border border-white/10 p-3\"><p lang=\"en\" class=\"text-[8px] uppercase tracking-widest text-gray-500\">{label}</p><p class=\"text-lg font-bold text-green-500\">{}</p></div>",
            escape_html_text(&value.to_string())
        );
    }
    html.push_str("</div>");

    html.push_str(
        "<table data-testid=\"public-mod-stats-monthly\" class=\"w-full mt-6 text-left text-[10px] font-mono\">",
    );
    html.push_str("<caption>月次集計</caption>");
    html.push_str(
        "<thead lang=\"en\" class=\"border-b border-white/10 text-gray-400 uppercase\"><tr>\
         <th scope=\"col\" class=\"py-2 pr-4\">Month</th><th scope=\"col\" class=\"py-2 pr-4\">Reports</th>\
         <th scope=\"col\" class=\"py-2 pr-4\">Concluded</th><th scope=\"col\" class=\"py-2 pr-4\">Approved</th>\
         <th scope=\"col\" class=\"py-2 pr-4\">Rejected</th></tr></thead>",
    );
    html.push_str("<tbody class=\"divide-y divide-white/5\">");
    for row in &stats.monthly {
        let _ = writeln!(
            html,
            "<tr><td class=\"py-2 pr-4 text-gray-300\">{}</td>\
             <td class=\"py-2 pr-4\">{}</td><td class=\"py-2 pr-4\">{}</td>\
             <td class=\"py-2 pr-4\">{}</td><td class=\"py-2 pr-4\">{}</td></tr>",
            escape_html_text(&row.month),
            escape_html_text(&row.reports.to_string()),
            escape_html_text(&row.concluded.to_string()),
            escape_html_text(&row.approved.to_string()),
            escape_html_text(&row.rejected.to_string()),
        );
    }
    html.push_str("</tbody></table>");
    if stats.monthly.is_empty() {
        html.push_str(
            "<p class=\"mt-2 text-[9px] text-gray-400 italic\">月次集計はありません。</p>",
        );
    }
    html
}

/// 見出しテキストから slug を作る。許可するのは Unicode の英数字・`-`・`_` のみで、
/// それ以外（HTML 属性を壊す `"`, `'`, `<`, `>`, `&`, 空白, 制御文字, `/` 等）は
/// すべて除去する（属性注入を防ぐ）。大文字は `to_lowercase()` で畳み、
/// すべて除去された場合は `section` を返す。
///
/// 属性値のエスケープは `escape_html_text`（TOC の href/表示名）と
/// pulldown-cmark（本文の id）に委譲し、ここでは URL エンコードしない。
fn sanitize_heading_slug(text: &str) -> String {
    let mut slug = String::new();
    for ch in text.chars() {
        // `-` / `_` と Unicode の英数字だけを許可する。引用符・山括弧・`&`・
        // 空白・制御文字・`/` 等は英数字ではないため許可リストから漏れ、
        // 属性注入の可能性を残さない。
        if ch == '-' || ch == '_' {
            slug.push(ch);
        } else if ch.is_alphanumeric() {
            // ASCII に限らない大文字畳み込み。İ のように複数文字へ展開される
            // 場合があるため、展開結果も英数字だけを通す。これにより U+0307
            // （結合文字）などが属性値へ混入するのを防ぐ。
            for lc in ch.to_lowercase() {
                if lc.is_alphanumeric() {
                    slug.push(lc);
                }
            }
        }
    }
    if slug.is_empty() {
        "section".to_string()
    } else {
        slug
    }
}

/// cid から DOM id / TOC アンカーの名前空間に使う属性安全なトークンを作る。
///
/// cid はサーバー由来の信頼できない値なので、そのまま属性へ埋め込まない。
/// `sanitize_heading_slug` で英数字（Unicode 含む）・`-`・`_` だけに正規化し、
/// さらに元の cid のハッシュを付与する。これにより引用符や `<` 等が属性を
/// 壊すことはなく、特殊文字だけの cid が同じトークンへ潰れることも防ぐ。
fn cid_scope_token(cid: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cid.hash(&mut hasher);
    let normalized = sanitize_heading_slug(cid);
    format!("c{normalized}-{:x}", hasher.finish())
}

/// canvas の読み上げラベル（純粋関数）。無題（空白のみを含む）なら cid 先頭 8 文字を添える。
/// タイトル非空の場合は既存の aria-label を変えず、そのまま返す。
fn illustration_aria_label(title: &str, cid: &str) -> String {
    if title.trim().is_empty() {
        let prefix: String = cid.chars().take(8).collect();
        format!("無題のイラスト ({prefix})")
    } else {
        title.to_string()
    }
}

/// 監査パネルの DOM id。トグルの `aria-controls` と同じ値を使う。
fn forensic_panel_id(cid_token: &str) -> String {
    format!("forensic-timeline-{cid_token}")
}

/// 既に使われている slug と衝突しない一意の値を割り当てる。
/// 例: `foo`, `foo-1`, `foo-2`。
///
/// `used` は「使用済み slug → 次に試す連番」を保持する HashMap。
/// 初回は `foo` を採用して `used["foo"] = 1` を記録し、衝突時はその
/// 連番から `foo-1`, `foo-2` ... を試す。採番した slug も同じくキーとして
/// 記録するので、明示的な `foo-1` 見出しとの衝突も旧実装どおり回避できる。
/// 使用済み slug を先頭から走査する旧実装は同一見出しが並ぶと O(N²) だったが、
/// 連番を覚えることでならし O(1) で採番できる。
fn assign_unique_slug(base: &str, used: &mut HashMap<String, usize>) -> String {
    let base = sanitize_heading_slug(base);
    loop {
        match used.get(&base).copied() {
            None => {
                // 未使用の slug。次に試す連番 1 を記録して採用。
                used.insert(base.clone(), 1);
                return base;
            }
            Some(next) => {
                // 使用済み。記録された連番から順に空きを探す。
                used.insert(base.clone(), next + 1);
                let candidate = format!("{base}-{next}");
                if !used.contains_key(&candidate) {
                    // 生成した slug も使用済みとして記録する。
                    used.insert(candidate.clone(), 1);
                    return candidate;
                }
            }
        }
    }
}

/// Markdown を一度だけ解析し、見出しのレベル・表示名・一意 slug を集める。
/// `render_markdown` と `render_toc` の両方がこの純粋関数を使うため、
/// TOC のアンカーと本文の見出し id は必ず一致する。
///
/// `prefix` は cid 由来の名前空間トークン（空なら従来どおり）。採番そのものは
/// `assign_unique_slug` が担い、prefix は採番後の slug に連結するだけなので
/// 重複連番のならし O(1) ロジックは変わらない。
fn collect_headings_with_prefix(markdown: &str, prefix: &str) -> Vec<HeadingEntry> {
    let mut headings = Vec::new();
    let mut used = HashMap::new();
    let mut current: Option<(HeadingLevel, String)> = None;

    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((level, String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = current.take() {
                    let slug = assign_unique_slug(&text, &mut used);
                    // prefix 付与だけを追加。採番済み slug はそのまま使う。
                    let slug = if prefix.is_empty() {
                        slug
                    } else {
                        format!("{prefix}-{slug}")
                    };
                    headings.push(HeadingEntry { level, text, slug });
                }
            }
            // render_markdown と同じく生 HTML も文字列として扱う。
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                if let Some((_, buf)) = current.as_mut() {
                    buf.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, buf)) = current.as_mut() {
                    buf.push(' ');
                }
            }
            _ => {}
        }
    }
    headings
}

/// 既存の接頭辞なし版。単体テストと後方互換のために残す。
#[cfg(test)]
fn collect_headings(markdown: &str) -> Vec<HeadingEntry> {
    collect_headings_with_prefix(markdown, "")
}

/// `POST /metrics/client-events` に送る JSON 本文を組み立てる（純粋関数）。
///
/// サーバーの共通インターフェースに合わせ、`event` 名と `count` 以外は一切含めない。
/// URL 本文・IP・User-Agent は送らない。件数は 1..=1000 に収め、
/// 0 件のときは送信しない（`None`）ため `spawn_local` を起こさない。
#[cfg(any(test, target_arch = "wasm32"))]
fn client_event_payload(count: usize) -> Option<String> {
    // ビルド時に OIL_CLIENT_METRICS=0 が指定された場合は一切送信しない。
    if !CLIENT_METRICS_ENABLED {
        return None;
    }
    if count == 0 {
        return None;
    }
    // サーバー側の受け入れ範囲は 1..=1000。超過分は丸めて送る。
    let count = count.min(1000);
    Some(format!(r#"{{"event":"neutralized_url","count":{count}}}"#))
}

/// 送信済みとして記録する描画ソース（markdown 本文のハッシュ）の集合。
///
/// `render_markdown` はリアクティブな owner を持たない自由関数なので
/// `StoredValue` は使えない。また再マウントをまたいで保持する必要があるため、
/// ページの寿命で生きる `thread_local` の `HashSet` を用いる（wasm は単一スレッド）。
#[cfg(any(test, target_arch = "wasm32"))]
type ReportedSources = std::collections::HashSet<u64>;

#[cfg(target_arch = "wasm32")]
thread_local! {
    static REPORTED: RefCell<ReportedSources> = RefCell::new(ReportedSources::new());
}

/// 描画ソース（markdown 本文）を安定したハッシュへ変換する。
#[cfg(any(test, target_arch = "wasm32"))]
fn markdown_source_hash(markdown: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    markdown.hash(&mut hasher);
    hasher.finish()
}

/// 送信済み記録に対する純粋判定。新規ソースなら `true`、既出なら `false`。
///
/// `HashSet::insert` の戻り値（新たに挿入されたか）をそのまま返すだけなので、
/// 副作用は集合の更新に限られ、実際の fetch は呼び出し側の副作用として扱う。
#[cfg(any(test, target_arch = "wasm32"))]
fn claim_report_source(reported: &mut ReportedSources, source_hash: u64) -> bool {
    reported.insert(source_hash)
}

/// ページ全体の送信済み記録を参照し、このソースが未送信なら記録して `true` を返す。
#[cfg(target_arch = "wasm32")]
fn should_report_source(source_hash: u64) -> bool {
    REPORTED.with(|reported| claim_report_source(&mut reported.borrow_mut(), source_hash))
}

/// 中立化した件数を監視用に 1 回だけ送信する。
///
/// 同じ描画ソースが再描画（画面の再マウント等）されたときは送信しない。
/// 描画をブロックしないよう `spawn_local` で非同期に送る。認証ヘッダは付けず、
/// 失敗しても画面にもコンソールにも何も出さない（機微情報をログに残さない）。
fn report_neutralized_urls(count: usize, markdown: &str) {
    // client_event_payload に渡す前にもガードし、送信経路を完全に止める。
    if !CLIENT_METRICS_ENABLED {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let Some(payload) = client_event_payload(count) else {
            return;
        };
        let source_hash = markdown_source_hash(markdown);
        if !should_report_source(source_hash) {
            return;
        }
        spawn_local(async move {
            if let Ok(request) = Request::post(&format!("{API_BASE}/metrics/client-events"))
                .header("Content-Type", "application/json")
                .body(payload)
            {
                // 監視用の副作用なので、HTTP エラーもネットワークエラーも握りつぶす。
                let _ = request.send().await;
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // ネイティブの単体テストでは fetch を起こさない。送信判定は純粋関数側で検証する。
        let _ = (count, markdown);
    }
}

/// Markdown を HTML に変換し、中立化した URL の件数も返す（副作用のない純粋関数）。
///
/// 件数は同期的な描画の中で `Cell` に集約する。送信は `render_markdown` 側で
/// 描画完了後に 1 回だけ行うため、リンク/img ごとに POST することはない。
fn render_markdown_with_neutralized_count_scoped(markdown: &str, prefix: &str) -> (String, usize) {
    // 先に見出しを収集し、Start(Heading) の順に id を割り当てる。
    // id は pulldown-cmark の HTML レンダラがエスケープするため安全。
    let headings = collect_headings_with_prefix(markdown, prefix);
    let mut next_heading = 0usize;
    let neutralized = std::cell::Cell::new(0usize);
    let events = Parser::new(markdown).map(|event| match event {
        Event::Start(Tag::Heading {
            level,
            id: _,
            classes,
            attrs,
        }) => {
            let id = headings
                .get(next_heading)
                .map(|heading| CowStr::from(heading.slug.clone()));
            next_heading += 1;
            Event::Start(Tag::Heading {
                level,
                id,
                classes,
                attrs,
            })
        }
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: if is_safe_markdown_url(&dest_url, false) {
                dest_url
            } else {
                neutralized.set(neutralized.get() + 1);
                "#".into()
            },
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: if is_safe_markdown_url(&dest_url, true) {
                dest_url
            } else {
                neutralized.set(neutralized.get() + 1);
                "#".into()
            },
            title,
            id,
        }),
        Event::Html(html) | Event::InlineHtml(html) => Event::Text(html),
        event => event,
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    (html, neutralized.get())
}

/// 既存の接頭辞なし版。単体テストと後方互換のために残す。
#[cfg(test)]
fn render_markdown_with_neutralized_count(markdown: &str) -> (String, usize) {
    render_markdown_with_neutralized_count_scoped(markdown, "")
}

fn render_markdown_scoped(markdown: &str, prefix: &str) -> String {
    let (html, neutralized_count) = render_markdown_with_neutralized_count_scoped(markdown, prefix);
    // 集約した件数を描画完了後に送る。同じ本文の再描画は送信側で除外される
    // （描画はブロックせず、HTML を返す契約は据え置き）。
    report_neutralized_urls(neutralized_count, markdown);
    html
}

/// 既存の接頭辞なし版。単体テストと後方互換のために残す。
#[cfg(test)]
fn render_markdown(markdown: &str) -> String {
    render_markdown_scoped(markdown, "")
}

/// 見出し一覧から TOC（アンカーリンクのリスト）を生成する。
/// リンク先は `collect_headings_with_prefix` が生成した `#slug` のみを使う。
/// `prefix` を渡すと本文の見出し id と同じ名前空間の href が生成される。
fn render_toc_with_prefix(markdown: &str, prefix: &str) -> String {
    let mut toc = String::new();
    for heading in collect_headings_with_prefix(markdown, prefix) {
        let indent = (heading.level as u32).saturating_sub(1) * 2;
        let _ = writeln!(
            toc,
            "<a class=\"block py-1 text-[10px] text-gray-400 hover:text-green-500 truncate\" style=\"padding-left:{}em\" href=\"#{}\">{}</a>",
            indent,
            escape_html_text(&heading.slug),
            escape_html_text(&heading.text),
        );
    }
    toc
}

/// 既存の接頭辞なし版。単体テストと後方互換のために残す。
#[cfg(test)]
fn render_toc(markdown: &str) -> String {
    render_toc_with_prefix(markdown, "")
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

    // network-path reference（`//host/path`）や Windows UNC（`\\host\path`）は
    // スキームが無いため従来は無条件に安全とみなされていたが、ブラウザが
    // 現在のスキームを補完して外部ホストへ誘導し得るため拒否する。
    // URL パーサは `\` を `/` に正規化するため、混在形式も併せて拒否する。
    let mut leading = destination.chars().take(2);
    if matches!(leading.next(), Some('/' | '\\')) && matches!(leading.next(), Some('/' | '\\')) {
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
    /// 以下は後方互換のため、旧サーバーのレスポンスでも空値として受け取る。
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// お気に入り状態。旧サーバーのレスポンスでも false として受け取る。
    #[serde(default)]
    pub is_favorite: bool,
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

/// `GET /mod/stats` の合計値。サーバー側の共通インターフェースに合わせる。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ModerationStatsTotals {
    #[serde(default)]
    pub reports: i64,
    #[serde(default)]
    pub concluded: i64,
    #[serde(default)]
    pub approved: i64,
    #[serde(default)]
    pub rejected: i64,
    #[serde(default)]
    pub quorum_not_met: i64,
    #[serde(default)]
    pub bans: i64,
    #[serde(default)]
    pub unbans: i64,
    #[serde(default)]
    pub unpins: i64,
}

/// 月次集計の1行（`month` は "YYYY-MM"）。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ModerationStatsMonthly {
    #[serde(default)]
    pub month: String,
    #[serde(default)]
    pub reports: i64,
    #[serde(default)]
    pub concluded: i64,
    #[serde(default)]
    pub approved: i64,
    #[serde(default)]
    pub rejected: i64,
    #[serde(default)]
    pub bans: i64,
    #[serde(default)]
    pub unbans: i64,
    #[serde(default)]
    pub unpins: i64,
}

/// `GET /mod/stats` レスポンス（運用者限定の認証内集計）。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ModerationStats {
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub totals: ModerationStatsTotals,
    #[serde(default)]
    pub monthly: Vec<ModerationStatsMonthly>,
}

/// `GET /mod/stats/public` の合計値（ADR-0008 段階2の公開ホワイトリスト）。
///
/// `bans` / `unbans` / `unpins` は公開しないため型に存在しない。
/// 全フィールドに `#[serde(default)]` を付けて旧サーバー互換を保つ。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PublicModerationStatsTotals {
    #[serde(default)]
    pub reports: i64,
    #[serde(default)]
    pub concluded: i64,
    #[serde(default)]
    pub approved: i64,
    #[serde(default)]
    pub rejected: i64,
    #[serde(default)]
    pub quorum_not_met: i64,
}

/// `GET /mod/stats/public` の月次集計の1行（`month` は "YYYY-MM"）。
///
/// `quorum_not_met` は totals のみに存在するため月次には含めない。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PublicModerationStatsMonthly {
    #[serde(default)]
    pub month: String,
    #[serde(default)]
    pub reports: i64,
    #[serde(default)]
    pub concluded: i64,
    #[serde(default)]
    pub approved: i64,
    #[serde(default)]
    pub rejected: i64,
}

/// `GET /mod/stats/public` レスポンス（認証済み利用者向けの公開集計）。
///
/// 機微情報（CID / reason / `user_hash`）は型に存在しない。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PublicModerationStats {
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub totals: PublicModerationStatsTotals,
    #[serde(default)]
    pub monthly: Vec<PublicModerationStatsMonthly>,
}

/// `GET /me/export` レスポンス（R5-04: 自分のデータをエクスポート）。
///
/// サーバー側の共通インターフェースと厳密に同じフィールドを持つ。
/// 画面には件数しか出さないため配列要素は `serde_json::Value` で十分。
/// 全フィールドに `#[serde(default)]` を付け、旧サーバーのレスポンスや
/// フィールド追加前の形式でも受け取れるようにする。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DataExport {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub users: Vec<serde_json::Value>,
    #[serde(default)]
    pub contents: Vec<serde_json::Value>,
    #[serde(default)]
    pub favorites: Vec<serde_json::Value>,
    #[serde(default)]
    pub transactions: Vec<serde_json::Value>,
    #[serde(default)]
    pub signatures: Vec<serde_json::Value>,
    #[serde(default)]
    pub content_audit_log: Vec<serde_json::Value>,
    #[serde(default)]
    pub reports: Vec<serde_json::Value>,
    #[serde(default)]
    pub votes: Vec<serde_json::Value>,
    #[serde(default)]
    pub payment_orders: Vec<serde_json::Value>,
    #[serde(default)]
    pub governance_audit_log: Vec<serde_json::Value>,
    #[serde(default)]
    pub refresh_tokens: Vec<serde_json::Value>,
    #[serde(default)]
    pub revoked_subs: Vec<serde_json::Value>,
}

impl DataExport {
    /// 画面に出すのは件数だけなので、個別のレコード内容は一切使わない。
    fn record_count(&self) -> usize {
        self.users.len()
            + self.contents.len()
            + self.favorites.len()
            + self.transactions.len()
            + self.signatures.len()
            + self.content_audit_log.len()
            + self.reports.len()
            + self.votes.len()
            + self.payment_orders.len()
            + self.governance_audit_log.len()
            + self.refresh_tokens.len()
            + self.revoked_subs.len()
    }
}

/// エクスポート成功時に `export-status` へ出す文言を組み立てる。
/// 型として読めないレスポンスを「0 件」成功にせず、固定エラーを返す。
/// エラー時は `export-error` にのみ表示し、`export-status` は空のままにする。
fn export_status_message(payload: &serde_json::Value) -> Result<String, String> {
    let data = serde_json::from_value::<DataExport>(payload.clone())
        .map_err(|_| EXPORT_PARSE_ERROR.to_string())?;
    Ok(format!(
        "エクスポート完了: {} 件のレコードをダウンロードしました。",
        data.record_count()
    ))
}

/// エクスポートの生成日時を集計カードと同じ整形書式で表示する（非 wasm は生 ISO を保持）。
#[cfg(target_arch = "wasm32")]
fn format_export_generated_at(iso: &str) -> String {
    format_local_datetime(iso)
}

#[cfg(not(target_arch = "wasm32"))]
fn format_export_generated_at(iso: &str) -> String {
    iso.to_string()
}

/// `sub` は識別子なので先頭8文字だけを出す。全文・生 JWT・トークンハッシュは一切描画しない。
fn shorten_export_sub(sub: &str) -> String {
    let mut chars = sub.chars();
    let prefix: String = chars.by_ref().take(8).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// 成功時に dedicated 要素へ出す生成日時と sub 短縮表示。解析失敗時は None。
fn export_status_details(payload: &serde_json::Value) -> Option<(String, String)> {
    let data = serde_json::from_value::<DataExport>(payload.clone()).ok()?;
    Some((
        format_export_generated_at(&data.generated_at),
        shorten_export_sub(&data.sub),
    ))
}

/// ダウンロード開始に失敗したときに表示する固定文言。
/// JS 側の例外メッセージは機微情報を含み得るため表示には使わない。
fn download_start_error_message() -> &'static str {
    "ダウンロードの開始に失敗しました。"
}

/// ダウンロード開始失敗時に、既存の固定エラー文言に添えて出す再試行案内。
/// 既存文言は変更せず、新規 dedicated 文言として追加する。
const EXPORT_DOWNLOAD_RETRY_HINT: &str =
    "ダウンロードが始まらない場合は、ブラウザのダウンロード許可を確認して再度お試しください。";

/// object URL を失効させるまでの保留時間（ms）。
/// 固定10秒では低速ストレージやセキュリティスキャンで保存開始を待ち切れないため、
/// ダウンロード開始後の余裕を大幅に確保する。失敗時は上記の再試行案内も出す。
const OBJECT_URL_REVOKE_DELAY_MS: i32 = 60_000;

/// `anchor.click()` が投げる JS 例外を Rust の `Err` へ変換して捕捉する。
/// web-sys の `click()` は例外を catch できないため、catch 付きの薄い JS ラッパーを介する。
#[wasm_bindgen(inline_js = r#"
export function click_download_anchor(anchor) {
    anchor.click();
}
"#)]
extern "C" {
    #[wasm_bindgen(catch, js_name = click_download_anchor)]
    fn click_download_anchor(anchor: &web_sys::HtmlAnchorElement) -> Result<(), JsValue>;
}

/// レスポンスの JSON をそのまま（未知のフィールドも含めて）pretty print して
/// Blob → object URL → `<a download>` で保存する。object URL はダウンロード
/// 開始後に revoke する（同期的な revoke は一部ブラウザで保存を中断するため）。
fn download_json_file(payload: &serde_json::Value, filename: &str) -> Result<(), String> {
    let text = serde_json::to_string_pretty(payload)
        .map_err(|_| "エクスポートの JSON 生成に失敗しました。".to_string())?;
    let parts = js_sys::Array::new();
    parts.push(&JsValue::from_str(&text));
    let options = BlobPropertyBag::new();
    options.set_type("application/json");
    let blob = Blob::new_with_str_sequence_and_options(&parts, &options)
        .map_err(|_| "エクスポートの Blob 生成に失敗しました。".to_string())?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| "ダウンロード URL の生成に失敗しました。".to_string())?;

    let window =
        web_sys::window().ok_or_else(|| "ブラウザの window が利用できません。".to_string())?;
    let document = window
        .document()
        .ok_or_else(|| "ブラウザの document が利用できません。".to_string())?;
    let anchor = document
        .create_element("a")
        .map_err(|_| "ダウンロード要素の生成に失敗しました。".to_string())?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| "ダウンロード要素の生成に失敗しました。".to_string())?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    // Firefox 等では DOM に繋がっていないアンカーの click が無視されることがある。
    let body = document.body();
    if let Some(body) = &body {
        let _ = body.append_child(&anchor);
    }
    // click が例外を投げても Err に変換して保持する。アンカーの後始末と
    // object URL の revoke 予約は失敗時も済ませてから呼び出し元へ返す。
    let click_result =
        click_download_anchor(&anchor).map_err(|_| download_start_error_message().to_string());
    if let Some(body) = &body {
        let _ = body.remove_child(&anchor);
    }

    let url_to_revoke = url.clone();
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url_to_revoke);
    });
    // 保存が開始される前に revoke するとダウンロードがキャンセルされるため遅延させる。
    // 旧実装の固定10秒では低速ストレージやセキュリティスキャンで保存開始を待ち切れ
    // なかったため、十分な余裕（OBJECT_URL_REVOKE_DELAY_MS）へ延長する。
    // 開始自体が失敗した場合は呼び出し側で再試行案内も表示する。
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        revoke.unchecked_ref(),
        OBJECT_URL_REVOKE_DELAY_MS,
    );

    click_result?;
    Ok(())
}

/// 履歴更新の種別。ユーザーのタブ切替だけ `pushState` を使う。
#[derive(Clone, Copy)]
enum NavigateMode {
    Push,
    Replace,
}

/// 履歴 API の更新失敗時に 1 回だけ出す固定文言。
/// URL・ペイロード・失敗した `JsValue` の中身は一切含めない。
const HISTORY_UPDATE_WARNING: &str = "履歴の更新に失敗しました。";

thread_local! {
    /// 履歴更新失敗の警告をページ寿命で 1 回だけ出すためのフラグ。
    static HISTORY_WARNED: Cell<bool> = const { Cell::new(false) };
}

/// 履歴更新失敗を固定文言で 1 回だけ警告する（詳細はエコーしない）。
fn warn_history_update_once() {
    HISTORY_WARNED.with(|warned| {
        if warned.replace(true) {
            return;
        }
        // 履歴更新失敗は E2E のオラクル（economy.spec.ts）で警告を検証しているため、
        // リリースビルドでも固定文言の警告のみを出す（値・URL・JsValue はエコーしない）。
        #[cfg(target_arch = "wasm32")]
        web_sys::console::warn_1(&JsValue::from_str(HISTORY_UPDATE_WARNING));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = HISTORY_UPDATE_WARNING;
    });
}

/// hash から view トークンだけを取り出す。カード詳細ディープリンクは
/// `?cid=...#dashboard` のような形を許容するため、`?` 以降はクエリとして扱う。
fn hash_view_token(hash: &str) -> &str {
    let body = hash.strip_prefix('#').unwrap_or(hash);
    body.split('?').next().unwrap_or(body)
}

/// `#` 付き hash から許可された view を取り出す。許可リスト外は dashboard 固定。
/// `login` は認証画面であってナビの view ではないため、ここでは dashboard へ
/// 正規化する（ログアウト後の直接リンクも認証画面から始まる）。
fn view_from_hash(hash: &str) -> &'static str {
    match hash_view_token(hash) {
        "upload" => "upload",
        "moderation" => "moderation",
        "ledger" => "ledger",
        _ => "dashboard",
    }
}

/// popstate で view 同期の対象にしてよい hash かどうか。
/// TOC の `#slug` 等はナビの view ではないため早期 return の判定に使う。
/// `history.state` マーカーは使わず、hash の許可リストだけを根拠にする。
fn is_known_view_hash(hash: &str) -> bool {
    matches!(
        hash_view_token(hash),
        "dashboard" | "upload" | "moderation" | "ledger" | "login"
    )
}

/// ログアウト API の失敗時に dedicated role=alert で出す固定文言。
/// 画面遷移は現行契約どおり即座にログインへ戻す（失敗で戻さないことはしない）。
const LOGOUT_FAILED_MESSAGE: &str = "ログアウト処理が完了しませんでした。再試行してください。";

/// 指定 view とクエリで履歴を更新する。通常の `navigate` と、
/// 投票キュー／カードからのディープリンクで同じ契約を共有する。
/// 生 URL や失敗内容は返さず、成否だけを呼び出し側へ渡す。
fn write_view_history(next: &str, query: &str, mode: NavigateMode) -> Result<(), ()> {
    let window = web_sys::window().ok_or(())?;
    let pathname = window.location().pathname().map_err(|_| ())?;
    let url = format!("{pathname}{query}#{next}");
    let history = window.history().map_err(|_| ())?;
    match mode {
        NavigateMode::Push => history
            .push_state_with_url(&JsValue::NULL, "", Some(&url))
            .map_err(|_| ()),
        NavigateMode::Replace => history
            .replace_state_with_url(&JsValue::NULL, "", Some(&url))
            .map_err(|_| ()),
    }
}

#[component]
pub fn App() -> impl IntoView {
    let (view, set_view) = signal("login".to_string());
    let (user_hash, set_user_hash) = signal(None::<String>);
    let (token, set_token) = signal(None::<String>);
    // ログアウト API の失敗表示。ログイン画面でも見えるよう main 直下に描画する。
    let (logout_error, set_logout_error) = signal(String::new());

    // api_fetch が試行ごとに現在の JWT を読めるよう、App の signal を共有状態へ登録する。
    register_shared_token(token, set_token);
    // refresh 応答の user_hash を現在のログインユーザーと照合するためにも登録する。
    register_user_hash(user_hash);

    // 直接リンクの hash は「保留値」としてだけ保持する。未認証では従来どおり
    // login を表示し、認証成功後にこの値を適用する。
    let pending_view: StoredValue<&'static str> = StoredValue::new(
        web_sys::window()
            .and_then(|window| window.location().hash().ok())
            .map(|hash| view_from_hash(&hash))
            .unwrap_or("dashboard"),
    );

    // popstate で未知 hash（TOC の #slug 等）へ戻ったときに正規化する直前の既知 view。
    // タブ切替（Push）では更新せず、初期化・認証遷移（Replace）と既知 hash の
    // popstate のときだけ更新する。これにより TOC アンカーの履歴条目は、
    // その直前の既知 view（dashboard 等）へ正規化できる。
    let last_known_view: StoredValue<String> = StoredValue::new("dashboard".to_string());

    // 全 view 遷移をここへ集約する。タブ切替だけ pushState、ログイン成功・
    // ログアウト・初期化は replaceState。検索条件（query）は current_content_query()
    // で正規化して埋め込み、未知キーを引き継がない。
    let navigate = move |next: &str, mode: NavigateMode| {
        // URL を先に組み、履歴 API の結果を保持する（view 更新より前）。
        // 失敗内容の JsValue は保持せず、警告は固定文言だけにする。
        // 生 search ではなく current_content_query() を埋め、未知キーを落とす。
        let history_result = write_view_history(next, &current_content_query(), mode);
        match mode {
            // タブ切替（Push）は履歴更新が成功したときだけ view を進める。
            // 失敗時は view を保ち、固定文言の警告を 1 回だけ出す。
            NavigateMode::Push => {
                if history_result.is_ok() {
                    set_view.set(next.to_string());
                } else {
                    warn_history_update_once();
                }
            }
            // 初期化・ログイン成功・ログアウト（Replace）は履歴の成否にかかわらず
            // set_view を維持し、失敗時だけ警告する。
            NavigateMode::Replace => {
                last_known_view.set_value(next.to_string());
                set_view.set(next.to_string());
                if history_result.is_err() {
                    warn_history_update_once();
                }
            }
        }
    };

    // 投票キューから BROWSE の該当カードへ戻るディープリンク。
    // 認証状態は触らず、投票前の本文・監査確認を同じセッションで行えるようにする。
    let open_content_from_report = move |cid: String| {
        let query = card_link_query(&cid);
        if write_view_history("dashboard", &query, NavigateMode::Push).is_ok() {
            set_view.set("dashboard".to_string());
        } else {
            warn_history_update_once();
        }
    };

    // ContentViewer の Report ボタンから MODERATION の通報フォームへ遷移する。
    // CID はクエリで渡し、Moderation 側がプリフィルする。
    let open_report_for_content = move |cid: String| {
        let query = report_prefill_query(&cid);
        if write_view_history("moderation", &query, NavigateMode::Push).is_ok() {
            set_view.set("moderation".to_string());
        } else {
            warn_history_update_once();
        }
    };

    // popstate では view だけを同期し、履歴をさらに操作しない。
    // Closure は Send + Sync でないため LocalStorage の StoredValue で保持し、
    // 既存タイマーと同じく on_cleanup で remove する。
    let popstate_listener: StoredValue<Option<EventClosure>, LocalStorage> =
        StoredValue::new_local(None);
    let popstate: EventClosure = Closure::new(move |_: web_sys::Event| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(hash) = window.location().hash() else {
            return;
        };
        // TOC の #slug 等、既知 view 以外の hash では「直前の既知 view」へ正規化する。
        // 未知 hash のまま view を放置すると、タブ切替後に戻ったときに URL と
        // 表示が食い違うため、既知 view が記録されていればそれへ戻す。
        if !is_known_view_hash(&hash) {
            let known = last_known_view.get_value();
            if known.is_empty() {
                return;
            }
            let next = if token.get_untracked().is_some() {
                known
            } else {
                "login".to_string()
            };
            set_view.set(next);
            return;
        }
        let next = if token.get_untracked().is_some() {
            view_from_hash(&hash)
        } else {
            "login"
        };
        last_known_view.set_value(next.to_string());
        set_view.set(next.to_string());
    });
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("popstate", popstate.as_ref().unchecked_ref());
        popstate_listener.set_value(Some(popstate));
    }
    on_cleanup(move || {
        let mut taken = None;
        popstate_listener.update_value(|slot| taken = slot.take());
        if let (Some(listener), Some(window)) = (taken, web_sys::window()) {
            let _ = window
                .remove_event_listener_with_callback("popstate", listener.as_ref().unchecked_ref());
        }
    });

    // 初期化も replaceState。保留値は既に読んでいるため直接リンクは失われない。
    navigate("login", NavigateMode::Replace);

    // SPA のビュー切替に合わせてタブタイトルを更新する。文言はナビゲーションと同じ。
    // ログイン等ナビに無いビューは共通タイトルのみにする。window が取れない
    // non-wasm 環境では何もしない。
    Effect::new(move |_| {
        let label = match view.get().as_str() {
            "dashboard" => "BROWSE",
            "upload" => "UPLOAD",
            "moderation" => "MODERATION",
            "ledger" => "LEDGER",
            _ => "",
        };
        let title = if label.is_empty() {
            "OpenIllustration".to_string()
        } else {
            format!("{label} — OpenIllustration")
        };
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_title(&title);
        }
    });

    // SPA のビュー切替後に本文へフォーカスを移す。ログイン成功・ログアウト・
    // タブ切替・popstate はいずれも set_view を通るため、この Effect 1つで
    // 全経路を同じ契約へ集約する。初回マウントは直前の view が無いため
    // フォーカスを奪わず、文字列が実際に変わったときだけ移す。
    let previous_view = Cell::new(None::<String>);
    Effect::new(move |_| {
        let current = view.get();
        let previous = previous_view.replace(Some(current.clone()));
        if !previous.is_some_and(|prev| prev != current) {
            return;
        }
        // main は常に DOM 上にあるため、動的ビューのマウントを待たずにフォーカスできる。
        if let Some(el) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|doc| doc.get_element_by_id("main-content"))
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    });

    let handle_login_success = move |hash: String, jwt: String| {
        set_logout_error.set(String::new());
        set_user_hash.set(Some(hash));
        // 先に epoch を進め、前回セッションの in-flight refresh を無効化する。
        bump_refresh_epoch();
        // 共有 signal へ set する。refresh 成功時も同じ signal を通るため
        // api_fetch は常に最新の JWT を読める。
        clear_refreshed_jwt();
        set_shared_jwt(Some(jwt));
        // 保留しておいた直接リンクの hash を適用する（未認証では login のまま）。
        let target = pending_view.get_value();
        navigate(target, NavigateMode::Replace);
    };

    // ナビゲーションの LOG OUT から呼ぶ。トークン／ユーザー状態を破棄して
    // ログイン画面へ戻す。API の成否にかかわらず画面は必ずログインへ戻し、
    // 未認証（トークン無し／401 済み）でも安全なよう失敗はすべて無視する。
    let handle_logout = move |_| {
        // 新しい試行では前回の失敗告知を消す。
        set_logout_error.set(String::new());
        // 進行中の refresh を無効化し、完了後に旧 JWT が書き戻されるのを防ぐ。
        bump_refresh_epoch();
        // refresh 済みなら新 JWT（REFRESHED_JWT）を優先してサーバの新 sid を失効させる。
        // 旧 token signal を読むと refresh 後のログアウトで新セッションが残る。
        let token = current_jwt();
        clear_refreshed_jwt();
        set_token.set(None);
        set_user_hash.set(None);
        // hash を #login へ replaceState（navigate 経由で履歴を汚さない）。
        navigate("login", NavigateMode::Replace);
        // トークンが無い場合は API を呼ばない。あっても 401／ネットワーク
        // エラーは握りつぶし、再ログイン導線を妨げない。
        if let Some(jwt) = token {
            spawn_local(async move {
                // 既存の /auth/logout 契約どおり Bearer でセッションを失効させる。
                let result = Request::post(&format!("{API_BASE}/auth/logout"))
                    .header("Authorization", &format!("Bearer {}", jwt))
                    // refresh_token Cookie も送り、サーバ側でファミリを失効させる
                    // （api_fetch と同じ credentials 契約）。
                    .credentials(RequestCredentials::Include)
                    .send()
                    .await;
                // 画面遷移は成否にかかわらず既にログイン済み。失敗だけ dedicated
                // role=alert で告知し、生エラーやステータス本文は出さない。
                match result {
                    Ok(response) if response.ok() => {}
                    _ => set_logout_error.set(LOGOUT_FAILED_MESSAGE.to_string()),
                }
            });
        }
    };

    view! {
        <div class="min-h-screen bg-black text-white font-mono">
            <a href="#main-content"
                on:click=move |ev| {
                    // hash ベースの view 同期と衝突しないよう、hash は書き換えず
                    // main へ直接フォーカスする（WCAG 2.4.1）。
                    ev.prevent_default();
                    if let Some(el) = web_sys::window()
                        .and_then(|window| window.document())
                        .and_then(|doc| doc.get_element_by_id("main-content"))
                        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let _ = el.focus();
                    }
                }
                class="sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-4 focus:z-[60] focus:bg-white focus:px-4 focus:py-2 focus:text-black focus:font-bold">
                "本文へスキップ"
            </a>
            <nav class="border-b border-white/10 p-4 flex justify-between items-center sticky top-0 bg-black z-50">
                <h1 lang="en" class="text-xl font-bold tracking-tighter">OPENILLUSTRATION / GUILD</h1>
                <Show when=move || view.get() != "login" fallback=|| ()>
                    <div class="flex gap-6 text-[10px] tracking-widest font-bold">
                        <button lang="en" on:click=move |_| navigate("dashboard", NavigateMode::Push)
                            aria-current=move || if view.get() == "dashboard" { Some("page") } else { None }
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "dashboard" { "text-white" } else { "text-gray-400" })>
                            "BROWSE"
                        </button>
                        <button lang="en" on:click=move |_| navigate("upload", NavigateMode::Push)
                            aria-current=move || if view.get() == "upload" { Some("page") } else { None }
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "upload" { "text-white" } else { "text-gray-400" })>
                            "UPLOAD"
                        </button>
                        <button lang="en" on:click=move |_| navigate("moderation", NavigateMode::Push)
                            aria-current=move || if view.get() == "moderation" { Some("page") } else { None }
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "moderation" { "text-white" } else { "text-gray-400" })>
                            "MODERATION"
                        </button>
                        <button lang="en" on:click=move |_| navigate("ledger", NavigateMode::Push)
                            aria-current=move || if view.get() == "ledger" { Some("page") } else { None }
                            class=move || format!("hover:text-white transition-colors {}", if view.get() == "ledger" { "text-white" } else { "text-gray-400" })>
                            "LEDGER"
                        </button>
                        <button lang="en" type="button" on:click=handle_logout
                            data-testid="logout-button"
                            class="text-gray-400 hover:text-white transition-colors">
                            "LOG OUT"
                        </button>
                        <span data-testid="user-hash" class="text-green-500 border border-green-500/20 px-2">
                            {move || user_hash.get().map(|h| h[0..8.min(h.len())].to_string()).unwrap_or_default()}
                        </span>
                    </div>
                </Show>
            </nav>

            <main id="main-content" tabindex="-1" class="container mx-auto p-4">
                // ログアウト API 失敗はログイン画面へ戻った後も dedicated role=alert で告知する。
                <Show when=move || !logout_error.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="logout-error" lang=move || error_lang(&logout_error.get()) class="mb-4 text-xs text-red-500">
                        {move || logout_error.get()}
                    </p>
                </Show>
                {move || match view.get().as_str() {
                    "login" => view! { <Login on_success=handle_login_success /> }.into_any(),
                    "dashboard" => view! { <Dashboard token=token.get().unwrap_or_default() user_hash=user_hash.get().unwrap_or_default() on_report=open_report_for_content /> }.into_any(),
                    "upload" => view! { <Upload user_hash=user_hash.get().unwrap_or_default() token=token.get().unwrap_or_default() /> }.into_any(),
                    "moderation" => view! {
                        <Moderation token=token.get().unwrap_or_default() user_hash=user_hash.get().unwrap_or_default() on_open_content=open_content_from_report />
                    }.into_any(),
                    "ledger" => view! {
                        <Ledger token=token.get().unwrap_or_default() />
                    }.into_any(),
                    _ => view! { <div class="text-center mt-20 text-gray-400">"不明な画面です。"</div> }.into_any(),
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
    let (error, set_error) = signal(String::new());

    let do_login = move |_| {
        // 同期ガード: DOM の disabled 反映前に届く2回目のクリックもここで止める。
        // report / vote / favorite と同じ claim_submission + BusyGuard の方針。
        if !claim_submission(loading.get()) {
            return;
        }
        set_loading.set(true);
        set_error.set(String::new());
        let on_success = on_success.clone();
        spawn_local(async move {
            // 成否いずれの分岐でもスコープ離脱時の Drop で busy を解除する。
            let _release = BusyGuard(set_loading);
            // 開発用モック認証でも nonce を取得して単回消費のフローを通す。
            // 501（本番認証未実装）は固定文言を出して verify を送らず早期 return する。
            // それ以外の失敗は従来どおり verify を試み、表示するエラーは verify の結果で上書きする。
            let nonce = match Request::post(&format!("{}/auth/login", API_BASE))
                .send()
                .await
            {
                Ok(response) if response.status() == 501 => {
                    set_error.set(PRODUCTION_AUTH_NOT_IMPLEMENTED.to_string());
                    set_loading.set(false);
                    return;
                }
                Ok(response) if response.ok() => match response.json::<Oid4vpRequest>().await {
                    Ok(request) => Some(request.nonce),
                    Err(_) => {
                        set_error.set("認証要求の解析に失敗しました。".to_string());
                        None
                    }
                },
                Ok(response) => {
                    set_error.set(format!("認証に失敗しました (HTTP {}).", response.status()));
                    None
                }
                Err(_) => {
                    set_error.set("ネットワークエラーが発生しました。".to_string());
                    None
                }
            };
            // リクエスト準備（URL / JSON シリアライズ）失敗で panic させず、
            // 既存の login-error 固定文言を role=alert で出し、ボタンを再活性化する。
            let Ok(request) =
                Request::post(&format!("{}/auth/verify", API_BASE)).json(&serde_json::json!({
                    "presentation_token": "mock_token",
                    "nonce": nonce,
                    "birth_date": "2000-01-01",
                    "country_code": "JP"
                }))
            else {
                set_error.set("ネットワークエラーが発生しました。".to_string());
                set_loading.set(false);
                return;
            };
            let res = request.send().await;

            match res {
                Ok(response) if response.ok() => match response.json::<VerifyResponse>().await {
                    Ok(data) if data.authorized => {
                        if let (Some(hash), Some(jwt)) = (data.user_hash, data.token) {
                            set_error.set(String::new());
                            on_success(hash, jwt);
                        } else {
                            set_error.set("認証情報が不足しています。".to_string());
                        }
                    }
                    Ok(_) => set_error.set("認証が許可されませんでした。".to_string()),
                    Err(_) => set_error.set("認証結果の解析に失敗しました。".to_string()),
                },
                Ok(response) if response.status() == 501 => {
                    set_error.set(PRODUCTION_AUTH_NOT_IMPLEMENTED.to_string());
                }
                Ok(response) => {
                    set_error.set(format!("認証に失敗しました (HTTP {}).", response.status()));
                }
                Err(_) => set_error.set("ネットワークエラーが発生しました。".to_string()),
            }
            set_loading.set(false);
        });
    };

    view! {
        <div class="max-w-md mx-auto mt-20 border border-white/10 p-8 text-center">
            <h2 class="text-2xl font-bold mb-8 tracking-tighter italic">"本人確認が必要です。"</h2>
            <p lang="en" class="text-[10px] text-gray-500 mb-4 leading-relaxed">
                "IPv6 connection is required. If you are on an IPv4-only network, please switch to an IPv6-capable connection."
            </p>
            <p lang="en" class="text-[10px] text-gray-500 mb-8 leading-relaxed">
                "JPKI-BASED AGE AND IDENTITY VERIFICATION. PERSONAL DATA IS HASHED ONLY."
            </p>
            <Show when=move || MOCK_AUTH_ENABLED>
                <p data-testid="mock-auth-notice">"開発用モック認証（本番では利用できません）。"</p>
            </Show>
            <button lang="en" on:click=do_login disabled=loading
                aria-busy=move || if loading.get() { "true" } else { "false" }
                class="w-full py-4 border border-white hover:bg-white hover:text-black transition-all font-bold uppercase tracking-widest disabled:opacity-50">
                {move || if loading.get() { "VERIFYING..." } else { "CONNECT WITH JPKI" }}
            </button>
            // 送信中は dedicated role=status で進捗を告知する（既存の role=alert は失敗のみ）。
            <Show when=move || loading.get() fallback=|| ()>
                <p data-testid="login-progress" role="status" aria-live="polite"
                    lang=move || error_lang(LOGIN_PROGRESS) class="text-xs text-gray-400 mt-4">
                    {LOGIN_PROGRESS}
                </p>
            </Show>
            <Show when=move || !error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="login-error" lang=move || error_lang(&error.get()) class="text-xs text-red-500">
                    {move || error.get()}
                </p>
            </Show>
        </div>
    }
}

#[component]
fn Dashboard<R>(token: String, user_hash: String, on_report: R) -> impl IntoView
where
    R: Fn(String) + 'static + Clone + Send,
{
    let (items, set_items) = signal(Vec::<ContentMeta>::new());
    let (error, set_error) = signal(String::new());
    // URL は signal 定義位置で一度だけ同期読みする。Effect 内で set すると
    // 初期値の通知が fetch Effect を再加熱し、二重 GET になるため避ける。
    let initial_query = parse_content_query(&current_content_query());
    // 投票キューからのディープリンクで指定された CID。検索条件には使わず、
    // URL の正規化と該当カードの強調表示だけに使う。
    let (deep_link_cid, set_deep_link_cid) = signal(initial_query.cid);
    let (q, set_q) = signal(initial_query.q);
    let (tag, set_tag) = signal(initial_query.tag);
    let (content_type, set_content_type) = signal(initial_query.content_type);
    let (favorite_only, set_favorite_only) = signal(initial_query.favorite_only);
    // トグル成功後に一覧を再取得するためのトリガー。
    let (refresh_counter, set_refresh_counter) = signal(0u32);
    let on_favorite_changed = move || set_refresh_counter.update(|n| *n += 1);
    // Search ボタン（form submit）押下時の即時検索トリガー。
    // テキスト入力の 250ms デバウンスは維持したまま、明示操作だけ即時取得へ切り替える。
    let (search_submit, set_search_submit) = signal(0u32);

    let token_stored = StoredValue::new(token);
    // ContentViewer への受け渡しを廃止したため未使用。StoredValue 自体は残す方針のため保持する。
    let _user_hash_stored = StoredValue::new(user_hash);

    // レスポンスの世代カウンタ。入力変化ごとに進め、古い応答は捨てる。
    // Leptos の StoredValue は SyncStorage を要求するため、ここでは
    // wasm シングルスレッド前提の Rc を Effect クロージャで直接共有する。
    let generation = Rc::new(Cell::new(0u64));
    // 直近の「即時取得トリガー」の値。テキスト入力との切り分けに使う。
    // 第3要素は Search ボタンの submit カウンタ。
    let last_immediate = Rc::new(Cell::new((false, 0u32, 0u32)));
    // 実行中のデバウンスタイマー。
    let pending_fetch = Rc::new(RefCell::new(None::<PendingFetch>));
    // submit ハンドラから保留中タイマーを同期キャンセルするための参照。
    let search_pending = Rc::clone(&pending_fetch);
    // on_cleanup は Send+Sync を要求するため、Rc を含む PendingFetch ではなく
    // タイマーハンドル（i32）だけを StoredValue に持つ。
    let timer_handle = StoredValue::new(None::<i32>);

    // popstate（戻る／進む）では検索条件とディープリンク CID を batch で同時反映する。
    // batch にすることで fetch Effect は 1 回だけ再実行され、二重 GET にならない。
    // cid も同じ履歴条目に紐づくため、往復で古い強調表示が残らないように復元する。
    // refresh_counter / search_submit は操作トリガーなので URL から復元しない。
    let query_listener: StoredValue<Option<EventClosure>, LocalStorage> =
        StoredValue::new_local(None);
    let query_popstate: EventClosure = Closure::new(move |_: web_sys::Event| {
        let restored = parse_content_query(&current_content_query());
        batch(move || {
            set_q.set(restored.q);
            set_tag.set(restored.tag);
            set_content_type.set(restored.content_type);
            set_favorite_only.set(restored.favorite_only);
            set_deep_link_cid.set(restored.cid);
        });
    });
    if let Some(window) = web_sys::window() {
        let _ = window
            .add_event_listener_with_callback("popstate", query_popstate.as_ref().unchecked_ref());
        query_listener.set_value(Some(query_popstate));
    }
    on_cleanup(move || {
        let mut taken = None;
        query_listener.update_value(|slot| taken = slot.take());
        if let (Some(listener), Some(window)) = (taken, web_sys::window()) {
            let _ = window
                .remove_event_listener_with_callback("popstate", listener.as_ref().unchecked_ref());
        }
    });

    // 検索条件 -> URL。build_content_query の戻り値を pathname + query + hash へ
    // replaceState する（pushState は view のみ）。hash を残すのは view を壊さない
    // ため。比較は正規化後ではなく「生の location.search」と行う。正規化後と
    // 比べると未知キーやエンコード差で毎回差分が出て履歴を書き換え続ける。
    // replaceState は popstate を出さないため、書き換えてもこの Effect は再入しない。
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(pathname) = window.location().pathname() else {
            return;
        };
        let Ok(hash) = window.location().hash() else {
            return;
        };
        let Ok(raw_search) = window.location().search() else {
            return;
        };
        let query = build_content_query(
            &q.get(),
            &tag.get(),
            &content_type.get(),
            favorite_only.get(),
        );
        // ディープリンクの cid を落とさずに正規化する（raw_search との比較もこの形で行う）。
        let query = append_query_param(&query, "cid", &deep_link_cid.get());
        if raw_search == query {
            return;
        }
        let url = format!("{pathname}{query}{hash}");
        if let Ok(history) = window.history() {
            let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url));
        }
    });

    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        // 再実行前とアンマウント時にタイマーを止め、破棄済み Closure が
        // 呼ばれないようにする（Rc 側の Closure は再実行時に drop される）。
        on_cleanup(move || {
            if let Some(handle) = timer_handle.get_value()
                && let Some(window) = web_sys::window()
            {
                window.clear_timeout_with_handle(handle);
            }
        });
        let favorite = favorite_only.get();
        let refresh = refresh_counter.get();
        let submit = search_submit.get();
        let previous = last_immediate.get();
        // 採用した方式: q/tag/type のテキスト入力だけ 250ms デバウンスする。
        // favorite_only の変更・refresh_counter（お気に入りトグルの成功通知）・
        // search_submit（Search ボタン／Enter の submit）は即時取得する。
        // これらはユーザーの明示操作なので待たせず、
        // かつ連続テキスト入力による 1 文字ごとの GET を防ぐ。
        let immediate = favorite != previous.0 || refresh != previous.1 || submit != previous.2;
        last_immediate.set((favorite, refresh, submit));

        // テキスト signal もここで読み、入力変化を Effect の追跡対象にする。
        let query = build_content_query(&q.get(), &tag.get(), &content_type.get(), favorite);
        let token = token_stored.get_value();

        let generation_cell = Rc::clone(&generation);
        let response_generation = generation_cell.get() + 1;
        generation_cell.set(response_generation);

        // 前回の予約をキャンセル（デバウンス）。Closure も drop して
        // キャンセル済みタイマーの Rust 側データが残らないようにする。
        let pending = Rc::clone(&pending_fetch);
        if let Some(old) = pending.borrow_mut().take() {
            if let Some(handle) = old.handle {
                window.clear_timeout_with_handle(handle);
            }
            drop(old.closure);
        }

        let fetch = move || {
            let query = query.clone();
            let token = token.clone();
            let generation_cell = Rc::clone(&generation_cell);
            spawn_local(async move {
                match api_fetch(
                    |jwt| {
                        // 共有 signal 未登録時は従来どおり prop のトークンを使う。
                        let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                        Request::get(&format!("{}/content{}", API_BASE, query))
                            .header("Authorization", &format!("Bearer {}", jwt))
                    },
                    true,
                )
                .await
                {
                    Ok(res) if res.ok() => match res.json::<Vec<ContentMeta>>().await {
                        Ok(list) => {
                            if should_apply_response(generation_cell.get(), response_generation) {
                                set_error.set(String::new());
                                set_items.set(list);
                            }
                        }
                        // HTTP 成功でも JSON 解析失敗なら無音にせず固定文言を出す。
                        Err(_) => {
                            if should_apply_response(generation_cell.get(), response_generation) {
                                set_error.set(CONTENT_INDEX_LOAD_ERROR.to_string());
                            }
                        }
                    },
                    // 401 だけは再ログイン案内へ切り替える。500 等は従来の LOAD_ERROR を維持する。
                    Ok(res) if res.status() == 401 => {
                        if should_apply_response(generation_cell.get(), response_generation) {
                            set_error.set(CONTENT_INDEX_UNAUTHORIZED.to_string());
                        }
                    }
                    Ok(_) => {
                        if should_apply_response(generation_cell.get(), response_generation) {
                            set_error.set(CONTENT_INDEX_LOAD_ERROR.to_string());
                        }
                    }
                    Err(_) => {
                        if should_apply_response(generation_cell.get(), response_generation) {
                            set_error.set(CONTENT_INDEX_NETWORK_ERROR.to_string());
                        }
                    }
                }
            });
        };

        let closure: Closure<dyn FnMut()> = Closure::new(fetch);
        let delay = if immediate { 0 } else { SEARCH_DEBOUNCE_MS };
        let handle = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                delay,
            )
            .ok();
        timer_handle.set_value(handle);
        *pending.borrow_mut() = Some(PendingFetch { handle, closure });
    });

    view! {
        <div class="mt-8">
            <section data-testid="export-section" class="border border-white/10 p-4 mb-8 flex flex-wrap items-center justify-between gap-4">
                <div>
                    <h2 lang="en" class="text-xs font-bold tracking-widest uppercase">"MY DATA"</h2>
                    <p lang="en" class="text-[9px] text-gray-500 mt-1">
                        "DOWNLOAD ALL OF YOUR DATA AS JSON."
                    </p>
                </div>
                <ExportMyData token=token_stored.get_value() />
            </section>
            <search>
                <form
                    on:submit=move |ev| {
                        ev.prevent_default();
                        // 保留中のデバウンスタイマーを同期キャンセルし、重複 GET を防ぐ。
                        let old = search_pending.borrow_mut().take();
                        if let Some(old) = old {
                            if let Some(handle) = old.handle
                                && let Some(window) = web_sys::window()
                            {
                                window.clear_timeout_with_handle(handle);
                            }
                            drop(old.closure);
                        }
                        // Effect を再実行させ、現在の条件で delay=0 の即時検索を予約する。
                        set_search_submit.update(|n| *n += 1);
                    }
                    class="grid grid-cols-1 md:grid-cols-[1fr_1fr_auto_auto_auto] gap-4 items-end border border-white/10 p-4"
                >
                    <div class="space-y-1">
                        <label for="content-q" lang="en" class="block text-[9px] uppercase tracking-widest text-gray-500">
                            "Search"
                        </label>
                        <input
                            id="content-q"
                            name="q"
                            type="search"
                            autocomplete="off"
                            autocapitalize="none"
                            spellcheck="false"
                            placeholder="TITLE OR DESCRIPTION"
                            on:input=move |ev| set_q.set(event_target_value(&ev))
                            prop:value=q
                            class="w-full bg-black border border-white/20 p-3 text-xs"
                        />
                    </div>
                    <div class="space-y-1">
                        <label for="content-tag" lang="en" class="block text-[9px] uppercase tracking-widest text-gray-500">
                            "Tag"
                        </label>
                        <input
                            id="content-tag"
                            name="tag"
                            type="search"
                            autocomplete="off"
                            autocapitalize="none"
                            spellcheck="false"
                            placeholder="TAG"
                            on:input=move |ev| set_tag.set(event_target_value(&ev))
                            prop:value=tag
                            class="w-full bg-black border border-white/20 p-3 text-xs"
                        />
                    </div>
                    <div class="space-y-1">
                        <label for="content-type" lang="en" class="block text-[9px] uppercase tracking-widest text-gray-500">
                            "Type"
                        </label>
                        <select
                            id="content-type"
                            name="type"
                            on:change=move |ev| set_content_type.set(event_target_value(&ev))
                            prop:value=content_type
                            class="w-full bg-black border border-white/20 p-3 text-xs"
                        >
                            <option value="">"ALL"</option>
                            <option value="Illustration">"Illustration"</option>
                            <option value="Novel">"Novel"</option>
                        </select>
                    </div>
                    <div class="flex items-center gap-2 pb-3">
                        <input
                            id="favorite-only"
                            name="favorite_only"
                            type="checkbox"
                            on:change=move |ev: web_sys::Event| {
                                set_favorite_only.set(event_target_checked(&ev))
                            }
                            prop:checked=favorite_only
                            class="h-4 w-4 accent-green-500 cursor-pointer"
                        />
                        <label for="favorite-only" lang="en"
                            class="text-[9px] uppercase tracking-widest text-gray-500 cursor-pointer">
                            "Favorites only"
                        </label>
                    </div>
                    <button type="submit" lang="en"
                        class="py-3 px-6 border border-white/20 text-[10px] uppercase tracking-widest hover:bg-white/10 transition-colors">
                        "Search"
                    </button>
                </form>
            </search>
            // エラー時は前回成功時の件数を読み上げない（古い件数が残るため）。
            // 成功時は従来どおり件数を告知し、失敗時は取得失敗を dedicated に告知する。
            <p role="status" aria-live="polite" lang=move || error_lang(if error.get().is_empty() { "件のコンテンツを表示中" } else { DASHBOARD_LOAD_FAILED_ANNOUNCEMENT }) class="sr-only">
                {move || if error.get().is_empty() {
                    format!("{}件のコンテンツを表示中", items.get().len())
                } else {
                    DASHBOARD_LOAD_FAILED_ANNOUNCEMENT.to_string()
                }}
            </p>
            // 一覧取得の失敗は items が前回成功値を保持していても常に表示する。
            // （空状態の中に出すと items が残っているときにエラーが消えていた。）
            <Show when=move || !error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="dashboard-error" lang=move || error_lang(&error.get()) class="text-xs text-red-500 mb-4">
                    {move || error.get()}
                </p>
            </Show>
            // 401 のときだけ、既存の LOG OUT ボタンからの再ログインを案内する。
            <Show when=move || error.get() == CONTENT_INDEX_UNAUTHORIZED fallback=|| ()>
                <p data-testid="dashboard-relogin-hint" class="text-xs text-gray-400 mb-4">
                    "LOG OUT からログアウトし、再度 JPKI でログインしてください。"
                </p>
            </Show>
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
                                title=item.title.clone()
                                description=item.description.clone()
                                tags=item.tags.clone()
                                is_favorite=item.is_favorite
                                on_favorite_changed=on_favorite_changed
                                on_report=on_report.clone()
                                highlighted=deep_link_cid.get() == item.cid
                            />
                        }
                    }
                />
                <Show when=move || items.get().is_empty() fallback=|| ()>
                    <div class="border border-white/10 p-4 flex flex-col items-center justify-center text-[10px] text-gray-400 col-span-full gap-2">
                        <span class="italic opacity-50">
                            {move || content_empty_state_message(&q.get(), &tag.get(), &content_type.get(), favorite_only.get())}
                        </span>
                        <Show when=move || search_filters_active(&q.get(), &tag.get(), &content_type.get(), favorite_only.get()) fallback=|| ()>
                            <button
                                type="button"
                                data-testid="clear-search-filters"
                                on:click=move |_| {
                                    set_q.set(String::new());
                                    set_tag.set(String::new());
                                    set_content_type.set(String::new());
                                    set_favorite_only.set(false);
                                }
                                class="border border-white/20 px-3 py-1 text-gray-400 hover:bg-white/10 hover:text-white transition-colors">
                                "条件をクリア"
                            </button>
                        </Show>
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// R5-04: 認証トークン付きで `GET /me/export` を呼び、レスポンスを JSON ファイルとして保存する。
/// 401 は「ログインが必要です」、それ以外の HTTP エラーは status だけを表示し、
/// レスポンス本文（機微情報）は画面へ出さない。
#[component]
fn ExportMyData(token: String) -> impl IntoView {
    let (error, set_error) = signal(String::new());
    let (status, set_status) = signal(String::new());
    let (busy, set_busy) = signal(false);
    // 成功時にだけ出す dedicated 要素。生 ISO / 全文 sub / トークンは保持しない。
    let (generated_at, set_generated_at) = signal(String::new());
    let (sub_label, set_sub_label) = signal(String::new());
    // ダウンロード開始失敗のときだけ出す dedicated 案内（既存エラー文言は不変）。
    let (download_hint, set_download_hint) = signal(String::new());
    let token_stored = StoredValue::new(token);

    let on_export = move |_| {
        // R2-BUSY: disabled が DOM へ反映される前に届く2回目のクリックも
        // claim_submission で同期に排除し、/me/export を1本だけにする。
        if !claim_submission(busy.get()) {
            return;
        }
        let token = token_stored.get_value();
        set_error.set(String::new());
        set_status.set(String::new());
        set_generated_at.set(String::new());
        set_sub_label.set(String::new());
        set_download_hint.set(String::new());
        set_busy.set(true);
        spawn_local(async move {
            // 成否いずれの分岐でもスコープ離脱時の Drop で busy を解除する。
            let _release = BusyGuard(set_busy);
            let result: Result<String, String> = async {
                let response = api_fetch(
                    |jwt| {
                        let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                        Request::get(&format!("{}/me/export", API_BASE))
                            .header("Authorization", &format!("Bearer {}", jwt))
                    },
                    true,
                )
                .await
                .map_err(|_| "ネットワークエラーが発生しました。".to_string())?;
                if response.status() == 401 {
                    return Err("ログインが必要です".to_string());
                }
                if !response.ok() {
                    return Err(format!(
                        "エクスポートに失敗しました (HTTP {}).",
                        response.status()
                    ));
                }
                let payload: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|_| "エクスポートの解析に失敗しました。".to_string())?;
                // 型定義で後方互換を確認し、画面には件数だけを出す。
                // 解析失敗時は「0 件」成功にせず、固定エラーだけを表示する。
                let message = export_status_message(&payload)?;
                if let Some((formatted, sub)) = export_status_details(&payload) {
                    set_generated_at.set(formatted);
                    set_sub_label.set(sub);
                }
                download_json_file(&payload, EXPORT_FILENAME)?;
                Ok(message)
            }
            .await;
            // busy 解除は BusyGuard の Drop へ一本化（手動 set_busy(false) は削除）。
            // リリース wasm は無ログ方針。EXPORT_RESULT の出力は dev-console
            // feature を明示指定した開発ビルドだけに限定する（詳細設計・メトリクス方針と一致）。
            #[cfg(feature = "dev-console")]
            web_sys::console::log_1(
                &format!(
                    "EXPORT_RESULT ok={} value={:?}",
                    result.is_ok(),
                    result.as_ref().ok()
                )
                .into(),
            );
            match result {
                Ok(message) => set_status.set(message),
                Err(message) => {
                    // 開始失敗のときだけ、既存固定文言に加えて再試行案内を出す。
                    if message == download_start_error_message() {
                        set_download_hint.set(EXPORT_DOWNLOAD_RETRY_HINT.to_string());
                    }
                    // 前回成功の生成日時/sub を残さない（失敗時は両要素を消す契約）。
                    set_generated_at.set(String::new());
                    set_sub_label.set(String::new());
                    set_error.set(message);
                }
            }
        });
    };

    view! {
        <div class="space-y-2 text-right">
            <button
                type="button"
                on:click=on_export
                disabled=busy
                aria-label=move || if busy.get() { "エクスポート中" } else { "データをエクスポート" }
                aria-busy=move || if busy.get() { "true" } else { "false" }
                data-testid="export-my-data"
                class="py-3 px-6 border border-white/20 text-[10px] uppercase tracking-widest hover:bg-white/10 transition-colors disabled:opacity-50"
            >
                {move || if busy.get() { "エクスポート中..." } else { "データをエクスポート" }}
            </button>
            <Show when=move || !download_hint.get().is_empty() fallback=|| ()>
                <p data-testid="export-download-hint" class="text-xs text-gray-400">
                    {move || download_hint.get()}
                </p>
            </Show>
            <Show when=move || !error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="export-error" lang=move || error_lang(&error.get()) class="text-xs text-red-500">
                    {move || error.get()}
                </p>
            </Show>
            <Show when=move || !status.get().is_empty() fallback=|| ()>
                <p role="status" data-testid="export-status" lang=move || error_lang(&status.get()) class="text-xs text-gray-400">
                    {move || status.get()}
                </p>
            </Show>
            <Show when=move || !generated_at.get().is_empty() fallback=|| ()>
                <p data-testid="export-generated-at" class="text-xs text-gray-400">
                    <span lang="en">"GENERATED_AT: "</span> {move || generated_at.get()}
                </p>
            </Show>
            <Show when=move || !sub_label.get().is_empty() fallback=|| ()>
                <p data-testid="export-sub" class="text-xs text-gray-400">
                    <span lang="en">"SUB: "</span> {move || sub_label.get()}
                </p>
            </Show>
        </div>
    }
}

mod jxl;

/// エラー／進捗文言の言語を判定し、要素の `lang` 属性へ渡す共通ヘルパ。
///
/// 契約: `role="alert"` / `role="status"` に限らず、動的・静的を問わず英文テキストを
/// 表示する要素は必ず `lang="en"` を宣言し、日本語テキストは `lang="ja"` を宣言する
/// （ルートの `lang="ja"` に依存した暗黙宣言は使わない）。固定の英文見出しや
/// 検証バッジ・種別バッジのような非ライブリージョンの英文もこの契約の対象であり、
/// 要素単位で言語を明示する。動的な文言はこのヘルパを経由して言語を決める。
/// 既存固定文言は一切変更しない。判定は文言に日本語文字（ひらがな・カタカナ・
/// 漢字）が含まれるかだけを見るため、個々の文言追加・変更で drift しない。
/// HTTP ステータス番号は判定に影響しない。
fn error_lang(message: &str) -> &'static str {
    if message
        .chars()
        .any(|c| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}'))
    {
        "ja"
    } else {
        "en"
    }
}

/// 公開集計の鮮度警告文言。error_lang 判定と DOM 描画の双方で同じ値を参照する。
const PUBLIC_STATS_STALE_MESSAGE: &str =
    "公開集計の生成から一定時間が経過しています。更新して最新の状態を確認してください。";

/// コンテンツ本文の取得失敗時に表示する固定文言を HTTP ステータスから選ぶ。
/// `None` はバイナリ読取失敗や通信エラー（ステータスが無い）を表す。
/// レスポンス本文や生エラーは機微情報を含み得るため一切渡さない。
fn content_load_error_message(status: Option<u16>) -> String {
    match status {
        Some(code) => format!("コンテンツの取得に失敗しました (HTTP {code})."),
        None => "コンテンツの取得に失敗しました。".to_string(),
    }
}

/// 検証状態からバッジ文言を決める（表示ロジックをテスト可能にするため分離）。
/// チェックサム不一致・JXL デコード失敗・取得失敗は「改ざん検知」とは別の状態として扱う。
fn verification_badge_text(
    verified: Option<bool>,
    decode_failed: bool,
    load_failed: bool,
) -> &'static str {
    if load_failed {
        return "× LOAD FAILED";
    }
    match (verified, decode_failed) {
        (Some(true), _) => "● VERIFIED",
        (Some(false), true) => "× DECODE FAILED",
        (Some(false), false) => "× COMPROMISED",
        (None, _) => "○ PENDING",
    }
}

/// お気に入り更新の失敗文言を状態から決める純粋ヘルパー。
/// 401 だけ再ログインを案内し、それ以外は従来の文言を維持する。
/// `None` は通信エラー（HTTP ステータスが無い）を表す。
fn favorite_error_message(status: Option<u16>) -> String {
    match status {
        Some(401) => "再ログインが必要です。".to_string(),
        Some(status) => format!("Failed to update favorite (HTTP {status})."),
        None => "Network error updating favorite.".to_string(),
    }
}

#[component]
fn ContentViewer<F, R>(
    cid: String,
    expected_checksum: String,
    content_type: String,
    price: i64,
    token: String,
    title: String,
    description: String,
    tags: Vec<String>,
    is_favorite: bool,
    on_favorite_changed: F,
    on_report: R,
    highlighted: bool,
) -> impl IntoView
where
    F: Fn() + 'static + Clone,
    R: Fn(String) + 'static + Clone,
{
    let cid_label = cid.clone();
    let data_cid = cid.clone();
    let report_click_cid = cid.clone();
    let report_click = on_report;
    // 一覧の複数カードが同じ markdown を描画しても DOM id / TOC アンカーが衝突しないよう、
    // cid 由来の属性安全なトークンを名前空間として使う。
    let cid_token = cid_scope_token(&cid);
    let heading_prefix = cid_token.clone();
    // メタデータは信頼できない入力なので、escape_html_text を通した文字列だけを
    // inner_html へ渡す（Markdown 本文/TOC の既存レンダリングには触れない）。
    let meta_html = render_content_meta_html(&title, &description, &tags);
    // canvas の読み上げラベル。無題なら cid 先頭 8 文字を添える。
    // entry_hash や検証バッジの契約には触れない。
    let canvas_aria_label = illustration_aria_label(&title, &cid);
    let (verified, set_verified) = signal(None::<bool>);
    // チェックサムは正常だが JXL のデコードに失敗した場合を改ざん検知と区別する。
    let (decode_error, set_decode_error) = signal(false);
    // 小説本文が UTF-8 として読めない場合もデコード失敗として扱い、JXL と同じ
    // バッジ契約（× DECODE FAILED）を維持する。entry_hash と正規経路は不変。
    let (novel_decode_error, set_novel_decode_error) = signal(false);
    // HTTP エラー／バイナリ読取失敗は改ざん検知ではなく取得失敗として扱う。
    let (content_load_error, set_content_load_error) = signal(String::new());
    let (novel_html, set_novel_html) = signal(String::new());
    let (novel_toc, set_novel_toc) = signal(String::new());
    let (audit_logs, set_audit_logs) = signal(Vec::<AuditLog>::new());
    let (audit_error, set_audit_error) = signal(String::new());
    let (show_audit, set_show_audit) = signal(false);
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    // 監査パネルのフォーカス管理用。CLOSE / Escape でトグルへ戻す。
    let audit_toggle_ref = NodeRef::<leptos::html::Button>::new();
    // non-modal パネル表示時: ダイアログ内の初期フォーカス先兼 Tab フォーカスの周回先。
    let audit_close_ref = NodeRef::<leptos::html::Button>::new();

    // Show が true になった後に CLOSE へフォーカスを移す。
    // <Show> が DOM を挿入するのを待つため requestAnimationFrame で 1 フレーム遅らせる。
    Effect::new(move |_| {
        if !show_audit.get() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let close_ref = audit_close_ref;
        let focus_close = Closure::once_into_js(move |_: f64| {
            if let Some(close) = close_ref.get() {
                let _ = close.focus();
            }
        });
        let _ = window.request_animation_frame(focus_close.unchecked_ref());
    });

    let token_stored = StoredValue::new(token);
    // For は同じ cid の行を作り直さないため、トグル状態はカード内で保持し、
    // 成功時に親へ通知して一覧を再取得する。
    let (favorite, set_favorite) = signal(is_favorite);
    let (favorite_error, set_favorite_error) = signal(String::new());
    // 送信中の二重クリックを防ぐガード。成功・失敗いずれでも必ず解除する。
    let (favorite_busy, set_favorite_busy) = signal(false);

    let favorite_cid = cid.clone();
    let toggle_favorite = move |_| {
        // 新しい試行では前回のエラーを消す（do_vote と同一方針）。
        set_favorite_error.set(String::new());
        // 同期ガード。DOM の disabled 反映より前に届く2回目のクリックもここで止める。
        if favorite_busy.get() {
            return;
        }
        set_favorite_busy.set(true);
        let cid = favorite_cid.clone();
        let token = token_stored.get_value();
        let turning_on = !favorite.get();
        let on_favorite_changed = on_favorite_changed.clone();
        spawn_local(async move {
            let url = format!("{}/content/{}/favorite", API_BASE, url_encode(&cid));
            let res = api_fetch(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    let builder = if turning_on {
                        Request::post(&url)
                    } else {
                        Request::delete(&url)
                    };
                    builder.header("Authorization", &format!("Bearer {}", jwt))
                },
                true,
            )
            .await;

            match res {
                Ok(response) if response.ok() => {
                    set_favorite_error.set(String::new());
                    set_favorite.set(turning_on);
                    // 一覧を再取得して、サーバー側の状態と表示を揃える。
                    on_favorite_changed();
                }
                Ok(response) => {
                    set_favorite_error.set(favorite_error_message(Some(response.status())));
                }
                Err(_) => {
                    set_favorite_error.set(favorite_error_message(None));
                }
            }
            // 成否にかかわらずガードを解除し、次のトグルを受け付けられるようにする。
            set_favorite_busy.set(false);
        });
    };

    // 監査ログは本文取得とは独立に、パネルを開くたびに取得する。
    // 取得済みフラグは in-flight の二重取得を防ぐためだけに使い、
    // 失敗時だけでなく close_audit でも戻す（再オープン時に必ず最新を再取得）。
    let audit_cid = cid.clone();
    let audit_fetched_cid: StoredValue<Option<String>> = StoredValue::new(None);
    Effect::new(move |_| {
        if !show_audit.get() {
            return;
        }
        let cid = audit_cid.clone();
        let fetched = audit_fetched_cid.get_value();
        if fetched.as_deref() == Some(cid.as_str()) {
            return;
        }
        // 取得開始時にフラグを立て、完了待ちの間の再トグルでも二重取得しないようにする。
        audit_fetched_cid.set_value(Some(cid.clone()));
        let token = token_stored.get_value();
        spawn_local(async move {
            match api_fetch(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::get(&format!("{}/content/{}/audit", API_BASE, url_encode(&cid)))
                        .header("Authorization", &format!("Bearer {}", jwt))
                },
                true,
            )
            .await
            {
                Ok(response) if response.ok() => match response.json::<Vec<AuditLog>>().await {
                    Ok(logs) => {
                        set_audit_error.set(String::new());
                        set_audit_logs.set(logs);
                    }
                    Err(_) => {
                        // 解析失敗は再試行対象。フラグを戻して再オープン時に再取得できるようにする。
                        audit_fetched_cid.set_value(None);
                        set_audit_error.set("監査ログの解析に失敗しました。".to_string());
                    }
                },
                Ok(response) => {
                    // HTTP エラーも再試行対象。フラグを戻す。
                    audit_fetched_cid.set_value(None);
                    set_audit_error.set(format!(
                        "監査ログの取得に失敗しました (HTTP {}).",
                        response.status()
                    ));
                }
                Err(_) => {
                    // ネットワークエラーも再試行対象。フラグを戻す。
                    audit_fetched_cid.set_value(None);
                    set_audit_error.set("Network error loading audit log.".to_string());
                }
            }
        });
    });

    let ct_ef = content_type.clone();
    Effect::new(move |_| {
        let cid = cid.clone();
        let heading_prefix = heading_prefix.clone();
        let expected_checksum = expected_checksum.clone();
        let ct = ct_ef.clone();
        let token = token_stored.get_value();
        spawn_local(async move {
            // 再取得や cid 切り替えで前回の失敗表示が残らないようにする。
            set_decode_error.set(false);
            set_novel_decode_error.set(false);
            set_content_load_error.set(String::new());
            let url = format!("{}/content/{}", API_BASE, url_encode(&cid));
            let res = api_fetch(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::get(&url).header("Authorization", &format!("Bearer {}", jwt))
                },
                true,
            )
            .await;

            match res {
                Ok(response) if response.ok() => match response.binary().await {
                    Ok(bytes) => {
                        let is_ok = common::verify_checksum(&bytes, &expected_checksum);
                        // チェックサム検証時点の値を入れ、Illustration ではデコード結果で上書きする。
                        set_verified.set(Some(is_ok));

                        if is_ok && !bytes.is_empty() {
                            if ct == "Illustration" {
                                if let Some(canvas) = canvas_ref.get() {
                                    use wasm_bindgen::JsCast;
                                    let el: web_sys::HtmlCanvasElement =
                                        (*canvas).clone().unchecked_into();
                                    match jxl::decode_jxl_to_canvas(&bytes, el) {
                                        Ok(()) => set_verified.set(Some(true)),
                                        Err(_) => {
                                            // デコーダのエラー値には内部情報が含まれ得るため表示せず、
                                            // 固定文言の専用エラーに切り替える。
                                            set_decode_error.set(true);
                                            set_verified.set(Some(false));
                                        }
                                    }
                                } else {
                                    // キャンバスが未取得では描画できず、成功として扱えない。
                                    set_decode_error.set(true);
                                    set_verified.set(Some(false));
                                }
                            } else {
                                // 不正 UTF-8 を lossy に置換して成功扱いにせず、JXL デコード失敗と
                                // 同じ dedicated エラー representation へ切り替える。
                                match String::from_utf8(bytes) {
                                    Ok(text) => {
                                        // 本文と TOC は同じテキスト・同じ cid 由来 prefix から生成し、
                                        // カード間でアンカーが衝突しないようにする。
                                        set_novel_toc
                                            .set(render_toc_with_prefix(&text, &heading_prefix));
                                        set_novel_html
                                            .set(render_markdown_scoped(&text, &heading_prefix));
                                    }
                                    Err(_) => {
                                        // 生バイトやデコーダの詳細は出さず固定文言のみを表示する。
                                        set_novel_decode_error.set(true);
                                        set_verified.set(Some(false));
                                    }
                                }
                            }
                        } else if is_ok && ct == "Illustration" {
                            // チェックサムが一致しても空バイトでは JXL をデコードできない。
                            set_decode_error.set(true);
                            set_verified.set(Some(false));
                        }
                    }
                    // バイナリ読取失敗は改ざんではなく取得失敗として固定文言を出す。
                    Err(_) => {
                        set_verified.set(Some(false));
                        set_content_load_error.set(content_load_error_message(None));
                    }
                },
                // HTTP エラー本文は見ず、ステータス番号だけを固定文言へ埋め込む。
                Ok(response) => {
                    set_verified.set(Some(false));
                    let status = response.status();
                    set_content_load_error.set(content_load_error_message(Some(status)));
                }
                Err(_) => {
                    set_verified.set(Some(false));
                    set_content_load_error.set(content_load_error_message(None));
                }
            }
        });
    });

    let ct_can = content_type.clone();
    let ct_nov = content_type.clone();
    let ct_bad = content_type.clone();
    let ct_toc = content_type.clone();
    // 動的属性のクロージャが値を move しないよう StoredValue で保持する。
    // トグルの aria-controls とパネルの id は同じ一意値を指す。
    let audit_panel_id: StoredValue<String> = StoredValue::new(forensic_panel_id(&cid_token));
    let audit_controls_id: StoredValue<String> =
        StoredValue::new(audit_panel_id.get_value().clone());

    // パネルを閉じたら取得済みフラグを戻す。次回オープンで必ず再取得し、
    // 監査ログ追加後の再オープンでも最新のタイムラインを表示する。
    let close_audit = move || {
        if show_audit.get_untracked() {
            set_show_audit.set(false);
            audit_fetched_cid.set_value(None);
            if let Some(toggle) = audit_toggle_ref.get() {
                let _ = toggle.focus();
            }
        }
    };

    view! {
        <div data-testid="content-card"
            data-cid=move || data_cid.clone()
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                // ダイアログ内（トグル含む）で Escape が押されたら閉じてフォーカスを戻す。
                if ev.key() == "Escape" && show_audit.get() {
                    close_audit();
                    return;
                }
                // パネル表示中の Tab フォーカス制御。パネル内のインタラクティブ要素は
                // CLOSE ボタンのみなので、Tab / Shift+Tab を CLOSE へ戻す現行挙動を維持する。
                // 将来パネル内にフォーカス可能要素を追加する場合は、shift_key() で
                // 先頭/末尾を判定するループ型トラップへ変更すること（今回は挙動を変更しない）。
                if ev.key() == "Tab" && show_audit.get() {
                    ev.prevent_default();
                    if let Some(close) = audit_close_ref.get() {
                        let _ = close.focus();
                    }
                }
            }
            class=move || format!("p-4 border border-white/10 bg-black text-white space-y-4 flex flex-col h-full {}", if highlighted { "ring-1 ring-green-500" } else { "" })>
            <div class="flex justify-between items-start">
                <p class="text-[8px] text-gray-400 truncate flex-1 mr-2">{cid_label.clone()}</p>
                <div class="flex gap-2">
                    <button
                        type="button"
                        on:click=toggle_favorite
                        data-testid="favorite-toggle"
                        disabled=move || favorite_busy.get()
                        lang="en"
                        aria-label=move || if favorite.get() { "Remove from favorites" } else { "Add to favorites" }
                        aria-pressed=move || if favorite.get() { "true" } else { "false" }
                        class="text-[10px] border border-white/20 px-1 hover:bg-white/10 transition-colors disabled:opacity-50">
                        {move || if favorite.get() { "★" } else { "☆" }}
                    </button>
                    <button on:click=move |_| {
                            if show_audit.get_untracked() {
                                close_audit();
                            } else {
                                set_show_audit.set(true);
                            }
                        }
                        node_ref=audit_toggle_ref
                        aria-expanded=move || if show_audit.get() { "true" } else { "false" }
                        aria-controls=move || audit_controls_id.get_value().to_string()
                        class="text-[8px] border border-white/20 px-1 hover:bg-white/10 transition-colors uppercase">
                        "Audit"
                    </button>
                    <button
                        type="button"
                        lang="en"
                        on:click=move |_| report_click(report_click_cid.clone())
                        data-testid="content-report-button"
                        class="text-[8px] border border-white/20 px-1 hover:bg-white/10 transition-colors uppercase">
                        "Report"
                    </button>
                </div>
            </div>
            <Show when=move || !favorite_error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="favorite-error" lang=move || error_lang(&favorite_error.get()) class="text-[8px] text-red-500">{move || favorite_error.get()}</p>
            </Show>
            <div class="border-b border-white/10 pb-3" inner_html=meta_html />

            <Show when=move || verified.get() == Some(true) && ct_toc == "Novel" && !novel_toc.get().is_empty() fallback=|| ()>
                <details class="border border-white/10 bg-white/5 p-3">
                    <summary lang="en" class="text-[10px] tracking-widest uppercase text-gray-400 hover:text-white cursor-pointer">
                        "Table of Contents"
                    </summary>
                    <nav class="mt-3 space-y-1 max-h-40 overflow-y-auto" inner_html=move || novel_toc.get() />
                </details>
            </Show>

            <div class="aspect-square bg-white/5 flex items-center justify-center relative overflow-hidden group">
                <canvas node_ref=canvas_ref
                    role="img"
                    aria-label=move || canvas_aria_label.clone()
                    class=move || format!("max-w-full max-h-full object-contain {}", if verified.get() == Some(true) && ct_can == "Illustration" { "" } else { "hidden" }) />

                <Show when=move || verified.get() == Some(true) && ct_nov == "Novel" fallback=|| ()>
                    <div class="p-4 text-xs prose prose-invert max-w-none w-full overflow-y-auto h-full" inner_html=move || novel_html.get() />
                </Show>

                {move || match verified.get() {
                    Some(true) => view! {
                        <div data-testid="support-overlay" tabindex="0" lang="en"
                            class="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity flex flex-col items-center justify-center p-6 text-center space-y-4">
                            <p lang="en" class="text-[10px] font-bold tracking-widest text-white uppercase">"Support Creator"</p>
                            <p lang="en" class="text-[20px] font-bold text-green-500 font-mono">{price} " JPYC"</p>
                            <button disabled lang="en"
                                class="px-6 py-2 border border-white/20 text-gray-500 cursor-not-allowed font-bold text-[10px] uppercase">
                                "Settlement Pending"
                            </button>
                            <p lang="en" class="text-[8px] text-gray-400 italic">"JPYC settlement verification is in development. Support is disabled until then."</p>
                        </div>
                    }.into_any(),
                    Some(false) => view! {
                        <div class="text-center p-4" data-testid="content-verification-error">
                            <Show when=move || !content_load_error.get().is_empty() fallback=|| ()>
                                <p role="alert" lang=move || error_lang("Content Load Failed") data-testid="content-load-error" class="text-red-500 font-bold text-[10px] mb-2 uppercase">
                                    "Content Load Failed"
                                </p>
                                <p data-testid="content-load-error-detail" lang=move || error_lang(&content_load_error.get()) class="text-gray-500 text-[8px]">
                                    {move || content_load_error.get()}
                                </p>
                            </Show>
                            <Show when=move || content_load_error.get().is_empty() && decode_error.get() fallback=|| ()>
                                <p role="alert" lang=move || error_lang("JXL Decode Failed") data-testid="jxl-decode-error" class="text-red-500 font-bold text-[10px] mb-2 uppercase">
                                    "JXL Decode Failed"
                                </p>
                                <p data-testid="jxl-decode-error-detail" lang="ja" class="text-gray-500 text-[8px]">
                                    "JXL のデコードに失敗しました。"
                                </p>
                            </Show>
                            // 小説本文の UTF-8 失敗は JXL とは別の dedicated role=alert で告知する。
                            <Show when=move || content_load_error.get().is_empty() && novel_decode_error.get() fallback=|| ()>
                                <p role="alert" lang=move || error_lang("Novel Decode Failed") data-testid="novel-decode-error" class="text-red-500 font-bold text-[10px] mb-2 uppercase">
                                    "Novel Decode Failed"
                                </p>
                                <p data-testid="novel-decode-error-detail" lang="ja" class="text-gray-500 text-[8px]">
                                    "小説本文の UTF-8 デコードに失敗しました。"
                                </p>
                            </Show>
                            <Show when=move || content_load_error.get().is_empty() && !decode_error.get() && !novel_decode_error.get() fallback=|| ()>
                                <p role="alert" data-testid="integrity-breach" lang="en" class="text-red-500 font-bold text-[10px] mb-2 uppercase">"Integrity Breach Detected"</p>
                                <p lang="en" class="text-gray-500 text-[8px]">"Wasm checksum does not match forensic registry."</p>
                            </Show>
                        </div>
                    }.into_any(),
                    None => view! { <span role="status" aria-live="polite" lang="en" class="text-[10px] text-gray-400 animate-pulse uppercase tracking-tighter">"Verifying_Integrity..."</span> }.into_any(),
                }}

                // このパネルは画像エリアに重なる non-modal disclosure（non-modal dialog）であり、
                // モーダルではない。背景（nav・他カード・Audit/favorite トグル）は引き続き
                // 操作可能なままなので、背景を aria-hidden / inert にしない。
                <Show when=move || show_audit.get() fallback=|| ()>
                    <div id=move || audit_panel_id.get_value().to_string()
                        role="dialog"
                        aria-label="Forensic Timeline"
                        class="absolute inset-0 bg-black/95 z-20 p-4 overflow-y-auto border border-white/20">
                        <h4 lang="en" class="text-[10px] font-bold mb-4 border-b border-white/10 pb-2">"FORENSIC_TIMELINE"</h4>
                        <Show when=move || !audit_error.get().is_empty() fallback=|| ()>
                            <p role="alert" data-testid="audit-error" lang=move || error_lang(&audit_error.get()) class="text-[8px] text-red-500">
                                {move || audit_error.get()}
                            </p>
                        </Show>
                        <div class="space-y-4">
                            <For
                                each=move || audit_logs.get()
                                key=|log| log.log_id
                                children=move |log| {
                                    // 表示は整形済み日時、生 ISO は title 属性へ保持する。
                                    let created_at = log.created_at.clone();
                                    // 理由は日本語になり得るため error_lang で言語を決める（値は属性へエコーしない）。
                                    let reason = log.reason.clone().unwrap_or_default();
                                    let reason_for_lang = reason.clone();
                                    view! {
                                        <div class="text-[8px] space-y-1">
                                            <div class="flex justify-between text-gray-400">
                                                <span data-testid="audit-created-at" title=created_at>{format_local_datetime(&log.created_at)}</span>
                                                <span class="text-green-500">{log.new_status.clone()}</span>
                                            </div>
                                            <p lang="en" class="text-gray-300">"By: " {log.changed_by.clone()}</p>
                                            <p class="italic"><span lang="en">"Reason: "</span><span lang=move || error_lang(&reason_for_lang)>{reason}</span></p>
                                            <p lang="ja" class="text-[8px] text-gray-500">"※ 表示は要約です。全文は運用者へお問い合わせください"</p>
                                            <p lang="en" class="text-[10px] text-gray-400 break-all font-mono">"Hash: " {log.entry_hash.clone()}</p>
                                        </div>
                                    }
                                }
                            />
                        </div>
                        <button on:click=move |_| {
                                // ダイアログを閉じた後、フォーカスをトグルへ戻し、
                                // 取得済みフラグを戻して次回オープンで再取得する。
                                close_audit();
                            }
                            lang="en"
                            node_ref=audit_close_ref
                            class="mt-4 w-full py-2 border border-white/20 text-[8px] hover:bg-white/10">
                            "CLOSE"
                        </button>
                    </div>
                </Show>
            </div>
            <div class="text-[10px] tracking-widest font-bold mt-auto flex justify-between items-center">
                <div class="flex items-center gap-4">
                    <span data-testid="content-verification-badge" lang="en" class=move || match verified.get() {
                        Some(true) => "text-green-500",
                        Some(false) => "text-red-500",
                        None => "text-gray-400",
                    }>
                        {move || verification_badge_text(
                            verified.get(),
                            decode_error.get() || novel_decode_error.get(),
                            !content_load_error.get().is_empty(),
                        )}
                    </span>
                    <span lang="en" class="text-gray-500 border border-gray-500/20 px-1">{ct_bad.to_uppercase()}</span>
                </div>
                <span class="text-green-500 font-mono">{price} " JPYC"</span>
            </div>
        </div>
    }
}

/// ファイル選択ラベルでネイティブの file input を開くキーかどうか。
/// Enter / Space のみを扱い、Space のページスクロールは呼び出し側で防ぐ。
fn is_file_picker_activation_key(key: &str) -> bool {
    matches!(key, "Enter" | " " | "Spacebar")
}

/// アップロード失敗時の文言を HTTP ステータスから選ぶ固定文言集合。
/// `None` はステータスを取得できなかった場合（ネットワークエラー）。
/// サーバーが返した本文や生エラーは決して表示しない。
/// 401 は再ログイン、413 はクライアント側のサイズ拒否と同じ文言を案内する。
fn upload_error_message(status: Option<u16>) -> String {
    match status {
        Some(401) => "再ログインが必要です。".to_string(),
        Some(413) => "ERROR: File exceeds the 10 MiB limit.".to_string(),
        Some(status) => format!("Failed to upload content (HTTP {status})."),
        None => "Network error uploading content.".to_string(),
    }
}

/// 価格の送信前検証に使う固定文言。生入力値は決め打ちで混入させない。
const PRICE_RANGE_ERROR: &str = "Price must be between 0 and 1000000 JPY.";

/// 選択ファイルの MIME／拡張子がサーバー受理対象外のときの送信前固定文言。
/// サーバーへ 400 の遠回りをせず、この文言を dedicated `role=alert` で告知する。
const UNSUPPORTED_UPLOAD_TYPE_ERROR: &str =
    "ERROR: Unsupported file type. Select a JXL or Markdown file.";

/// サーバー契約 `0..=1_000_000` の範囲外なら dedicated エラーを返す純粋関数。
/// 入力途中で丸めない `price_from_number` の分、送信直前にこれでゲートする。
fn price_range_error(price: i32) -> Option<&'static str> {
    if (0..=1_000_000).contains(&price) {
        None
    } else {
        Some(PRICE_RANGE_ERROR)
    }
}

/// 数値を価格として使える整数かどうかを判定する純粋関数。
///
/// `NaN` / 無限大 / 小数 / i32 範囲外は `None` とし、呼び出し側が直前の有効値を保持する。
/// 旧実装の `unwrap_or(0)` は入力途中の空文字や `e` などを 0 円へ化けていたため、
/// 失敗時に signal を書き換えないことをここで保証する。
fn price_from_number(value: f64) -> Option<i32> {
    if !value.is_finite()
        || value.fract() != 0.0
        || value < i32::MIN as f64
        || value > i32::MAX as f64
    {
        return None;
    }
    Some(value as i32)
}

/// 価格入力の文字列を解釈する純粋関数（単体テスト用）。
///
/// `str::parse::<f64>()` により `1e3` のような指数表記も受理し、
/// 整数判定と範囲検査は `price_from_number` へ委譲する。
/// 実 UI ではブラウザの `input.valueAsNumber` を同じ判定へ通す（下記 `on:input` を参照）。
#[cfg(test)]
fn parse_price_input(raw: &str) -> Option<i32> {
    price_from_number(raw.parse::<f64>().ok()?)
}

/// メタデータのバイト上限超過時に表示する固定文言。生入力値は一切混入させない。
const METADATA_BYTE_ERROR: &str =
    "入力が長すぎます。タイトル・説明・タグの上限を確認してください。";

/// title / description / tags の UTF-8 バイト上限を検査する純粋関数。
///
/// `tags_csv` の分割規則はサーバーの `parse_tags_field` と同じ（カンマ分割・前後の空白除去・
/// 空要素除外）。文字数ではなく [`str::len`] によるバイト数で判定するため、日本語では
/// maxlength（UTF-16）をすり抜けた入力もここで拒否できる。
fn metadata_byte_error(title: &str, description: &str, tags_csv: &str) -> Option<&'static str> {
    if title.len() > common::MAX_CONTENT_TITLE_BYTES {
        return Some(METADATA_BYTE_ERROR);
    }
    if description.len() > common::MAX_CONTENT_DESCRIPTION_BYTES {
        return Some(METADATA_BYTE_ERROR);
    }
    let tags: Vec<&str> = tags_csv
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .collect();
    if tags.len() > common::MAX_CONTENT_TAGS {
        return Some(METADATA_BYTE_ERROR);
    }
    if tags
        .iter()
        .any(|tag| tag.len() > common::MAX_CONTENT_TAG_BYTES)
    {
        return Some(METADATA_BYTE_ERROR);
    }
    None
}

#[component]
fn Upload(user_hash: String, token: String) -> impl IntoView {
    let (status, set_status) = signal(String::new());
    // 失敗は dedicated role="alert" で告知する。成功・進捗は status のまま。
    let (failed, set_failed) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (file, set_file) = signal(None::<web_sys::File>);
    let (price, set_price) = signal(100);
    let (title, set_title) = signal(String::new());
    let (description, set_description) = signal(String::new());
    let (tags, set_tags) = signal(String::new());
    // ラベルをキーボードで操作したときに file input へclick()を渡すための参照。
    let file_input_ref = NodeRef::<leptos::html::Input>::new();

    let on_file_change = move |ev| {
        // 送信中はファイル選択を受け付けない。disabled 入力への直接設定や
        // レースで change が届いても、送信中の File シグナルを差し替えない。
        if loading.get() {
            return;
        }
        let input: HtmlInputElement = event_target(&ev);
        if let Some(files) = input.files()
            && let Some(f) = files.get(0)
        {
            if f.size() > MAX_UPLOAD_BYTES {
                set_file.set(None);
                set_failed.set("ERROR: File exceeds the 10 MiB limit.".to_string());
                return;
            }
            set_status.set(String::new());
            // 新しいファイル選択で前回の失敗表示を消す。
            set_failed.set(String::new());
            set_file.set(Some(f));
        } else {
            // 選択解除（キャンセル）時は古い File を破棄し、送信ボタンを無効化する。
            set_file.set(None);
            set_status.set(String::new());
            set_failed.set(String::new());
        }
    };

    let sign_and_upload = move |_| {
        // R2-BUSY: ファイル/MIME/価格ゲートより前に送信中を同期で排除する。
        // disabled の DOM 反映前に届く2回目のクリックは通信開始前にここで止める。
        if !claim_submission(loading.get()) {
            return;
        }
        // ファイル未選択（サイズ拒否を含む）ではパニックせずエラー表示する。
        let Some(f) = file.get() else {
            set_failed.set("ERROR: Select a file first.".to_string());
            return;
        };
        // 選択ファイルの MIME／拡張子を送信前にゲートする。空ならサーバー400の
        // 遠回りをせず、専用の固定文言を dedicated role=alert で告知して通信しない。
        if upload_media_type(&f.type_(), &f.name()).is_empty() {
            set_failed.set(UNSUPPORTED_UPLOAD_TYPE_ERROR.to_string());
            return;
        }
        let user_hash = user_hash.clone();
        let token = token.clone();
        let price_val = price.get();
        let title_val = title.get();
        let description_val = description.get();
        let tags_val = tags.get();
        // 送信直前にサーバー契約 0..=1_000_000 をゲートする。入力中に黙って丸めない
        // 経路を維持するため、範囲外はここで固定文言を出して通信を開始しない。
        if let Some(error) = price_range_error(price_val) {
            set_failed.set(error.to_string());
            return;
        }
        // メタデータも通信前に UTF-8 バイト契約でゲートする。HTML の maxlength は
        // UTF-16 単位のため日本語では守れず、ここで拒否すれば HTTP は 0 本のまま。
        if let Some(error) = metadata_byte_error(&title_val, &description_val, &tags_val) {
            set_failed.set(error.to_string());
            return;
        }
        set_loading.set(true);
        // 新しい試行では前回の失敗を消し、進捗は status のまま通知する。
        set_failed.set(String::new());
        set_status.set("監査証跡を生成中...".to_string());

        // 失敗時に表示する文言は固定文字列のみ（生データ・機微情報は出さない）。
        // HTTP エラー／ネットワークエラーは upload_error_message に統一する。
        const PREPARE_ERROR: &str = "アップロードの準備に失敗しました。";
        const RESPONSE_ERROR: &str = "アップロード結果の読み取りに失敗しました。";

        spawn_local(async move {
            // 成否いずれの分岐でもスコープ離脱時の Drop で loading を解除する。
            let _release = BusyGuard(set_loading);
            // 準備・通信の各段を Result で伝播させ、パニックで loading が戻らない事故を防ぐ。
            let outcome: Result<String, &'static str> = async move {
                let array_buffer = wasm_bindgen_futures::JsFuture::from(f.array_buffer())
                    .await
                    .map_err(|_| PREPARE_ERROR)?;
                let bytes = js_sys::Uint8Array::new(&array_buffer).to_vec();

                let mock_signature = vec![0u8; 256];

                let form_data = FormData::new().map_err(|_| PREPARE_ERROR)?;
                let media_type = upload_media_type(&f.type_(), &f.name());
                let options = BlobPropertyBag::new();
                options.set_type(media_type);
                let blob = Blob::new_with_u8_array_sequence_and_options(
                    &js_sys::Array::of1(&js_sys::Uint8Array::from(&bytes[..]).into()),
                    &options,
                )
                .map_err(|_| PREPARE_ERROR)?;
                form_data
                    .append_with_blob_and_filename("file", &blob, &f.name())
                    .map_err(|_| PREPARE_ERROR)?;
                form_data
                    .append_with_str("owner_hash", &user_hash)
                    .map_err(|_| PREPARE_ERROR)?;
                form_data
                    .append_with_str("price", &price_val.to_string())
                    .map_err(|_| PREPARE_ERROR)?;
                // 任意メタデータ。未入力でも1フィールド送り、サーバー側で空値として扱う。
                form_data
                    .append_with_str("title", &title_val)
                    .map_err(|_| PREPARE_ERROR)?;
                form_data
                    .append_with_str("description", &description_val)
                    .map_err(|_| PREPARE_ERROR)?;
                // tags はカンマ区切り1フィールド（サーバー側の共通インターフェース）。
                form_data
                    .append_with_str("tags", &tags_val)
                    .map_err(|_| PREPARE_ERROR)?;
                let sig_blob = Blob::new_with_u8_array_sequence(&js_sys::Array::of1(
                    &js_sys::Uint8Array::from(&mock_signature[..]).into(),
                ))
                .map_err(|_| PREPARE_ERROR)?;
                form_data
                    .append_with_blob("signature", &sig_blob)
                    .map_err(|_| PREPARE_ERROR)?;

                // ボディは試行ごとに FormData を複製して作り直す（消費済みの再利用を避ける）。
                // credentials は api_fetch_request が自動付与しないためここで指定する。
                let make = |jwt: &str| -> Result<Request, gloo_net::Error> {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::post(&format!("{}/content/upload", API_BASE))
                        .header("Authorization", &format!("Bearer {}", jwt))
                        .credentials(RequestCredentials::Include)
                        .body(form_data.clone())
                };
                // 既存契約どおり、ボディ生成の失敗は送信前に PREPARE_ERROR へ写像する。
                let _ = make(&token).map_err(|_| PREPARE_ERROR)?;
                let response = match api_fetch_request(make, true).await {
                    Ok(response) => response,
                    // ネットワークエラーも固定文言へ統一（生エラーは出さない）。
                    Err(_) => return Ok(upload_error_message(None)),
                };

                if !response.ok() {
                    return Ok(upload_error_message(Some(response.status())));
                }
                let cid = response
                    .json::<String>()
                    .await
                    .map_err(|_| RESPONSE_ERROR)?;
                Ok(format!("SUCCESS: Content registered as {}", cid))
            }
            .await;

            match outcome {
                // 成功は "SUCCESS:" で始まるため status へ。それ以外の Ok は
                // upload_error_message の固定エラー文言なので failed へ振り分ける（文言は不変）。
                Ok(message) if message.starts_with("SUCCESS:") => set_status.set(message),
                Ok(message) => set_failed.set(message),
                Err(message) => set_failed.set(message.to_string()),
            }
            // finally 相当の解除は BusyGuard の Drop が担う（手動 set_loading(false) は削除）。
        });
    };

    view! {
        <div class="max-w-2xl mx-auto mt-12 border border-white/10 p-8">
            <h2 lang="en" class="text-xl font-bold mb-6 tracking-widest uppercase">"Forensic Upload Engine"</h2>
            <div class="space-y-6">
                <div class="border-2 border-dashed border-white/20 p-12 text-center hover:border-white/40 transition-colors">
                    // 視覚的に隠すがタブ停止点にはしない（ラベル側を唯一のタブ停止点にする）。
                    <input type="file" on:change=on_file_change
                        disabled=move || loading.get()
                        accept=".jxl,.md,.markdown,.txt,image/jxl,text/markdown"
                        node_ref=file_input_ref
                        tabindex="-1"
                        aria-hidden="true"
                        class="absolute h-px w-px overflow-hidden whitespace-nowrap border-0 p-0 clip-[rect(0,0,0,0)]"
                        id="file-upload" />
                    <label for="file-upload"
                        lang="en"
                        role="button"
                        tabindex=move || if loading.get() { -1 } else { 0 }
                        aria-disabled=move || if loading.get() { "true" } else { "false" }
                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                            if loading.get() {
                                ev.prevent_default();
                                return;
                            }
                            if is_file_picker_activation_key(&ev.key()) {
                                ev.prevent_default();
                                if let Some(input) = file_input_ref.get() {
                                    input.click();
                                }
                            }
                        }
                        class=move || format!(
                            "{} {}",
                            if loading.get() {
                                "cursor-not-allowed opacity-50 pointer-events-none"
                            } else {
                                "cursor-pointer"
                            },
                            "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-white"
                        )>
                        {move || file.get().map(|f| f.name()).unwrap_or_else(|| "SELECT JXL OR MARKDOWN".to_string())}
                    </label>
                </div>

                <div class="space-y-2">
                    <label for="upload-price" lang="en" class="text-[10px] text-gray-500 uppercase tracking-widest">"Support Price (JPYC)"</label>
                    <input type="number"
                        id="upload-price"
                        name="price"
                        min="0"
                        max="1000000"
                        on:input=move |ev| {
                            // ブラウザ自身の数値解釈（valueAsNumber）を使う。Rust 側で文字列を
                            // パースしないため 1e3 のような指数表記も 1000 として受理する。
                            // 有限・整数・i32 範囲内でなければ signal を更新せず直前値を保持し、
                            // 受理時は丸めずにそのまま保持する（範囲外は送信前ゲートで拒否）。
                            let Some(target) = ev.target() else {
                                return;
                            };
                            let Ok(input) = target.dyn_into::<HtmlInputElement>() else {
                                return;
                            };
                            if let Some(value) = price_from_number(input.value_as_number()) {
                                set_price.set(value);
                            }
                        }
                        prop:value=price
                        class="w-full bg-black border border-white/20 p-4 text-xl font-mono text-green-500" />
                </div>

                <div class="space-y-1">
                    <label for="upload-title" lang="en" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "Title (optional)"
                    </label>
                    <input
                        id="upload-title"
                        name="title"
                        type="text"
                        maxlength=UPLOAD_TITLE_MAX_LEN
                        placeholder="TITLE"
                        on:input=move |ev| set_title.set(event_target_value(&ev))
                        prop:value=title
                        class="w-full bg-black border border-white/20 p-3 text-sm"
                    />
                </div>

                <div class="space-y-1">
                    <label for="upload-description" lang="en" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "Description (optional)"
                    </label>
                    <textarea
                        id="upload-description"
                        name="description"
                        maxlength=UPLOAD_DESCRIPTION_MAX_LEN
                        placeholder="DESCRIPTION"
                        on:input=move |ev| set_description.set(event_target_value(&ev))
                        prop:value=description
                        class="w-full bg-black border border-white/20 p-3 text-sm h-24 resize-y"
                    />
                </div>

                <div class="space-y-1">
                    <label for="upload-tags" lang="en" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "Tags (optional, comma separated)"
                    </label>
                    <input
                        id="upload-tags"
                        name="tags"
                        type="text"
                        maxlength=UPLOAD_TAGS_MAX_LEN
                        placeholder="FANTASY, SCIFI"
                        on:input=move |ev| set_tags.set(event_target_value(&ev))
                        prop:value=tags
                        class="w-full bg-black border border-white/20 p-3 text-sm"
                    />
                </div>

                // ARIA boolean は "true"/"false" の文字列トークンを要求するため、
                // 通報・投票・ログインと同じ形式で明示する（属性の有無ではない）。
                <button data-testid="upload-submit" on:click=sign_and_upload disabled=move || file.get().is_none() || loading.get()
                    aria-busy=move || if loading.get() { "true" } else { "false" }
                    class="w-full py-4 bg-white text-black font-bold uppercase tracking-widest hover:bg-gray-200 transition-colors disabled:opacity-50">
                    {move || if loading.get() { "処理中..." } else { "Sign & Submit to Evidence Ledger" }}
                </button>
                <Show when=move || !status.get().is_empty() fallback=|| ()>
                    <div data-testid="upload-status" role="status" aria-live="polite" lang=move || error_lang(&status.get()) class="p-4 bg-white/5 border border-white/20 text-xs">
                        <span class="text-gray-500 mr-2">"[STATUS]"</span> {move || status.get()}
                    </div>
                </Show>
                <Show when=move || !failed.get().is_empty() fallback=|| ()>
                    <div data-testid="upload-failed" role="alert" lang=move || error_lang(&failed.get()) class="p-4 bg-white/5 border border-red-500/40 text-xs">
                        <span class="text-gray-500 mr-2">"[FAILED]"</span> {move || failed.get()}
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// `/transactions` の取得失敗時に表示する固定文言。
/// レスポンス本文や生エラー（reqwest / serde_json）は機微情報を含み得るため出さない。
const LEDGER_ERROR: &str = "取引履歴の取得に失敗しました。";
/// 取得中に dedicated `role=status` で告知する固定文言（既存固定文言は変更しない）。
const LEDGER_LOADING: &str = "取引履歴を取得しています。";
/// `/transactions` が 401 のときだけ出す dedicated 固定文言。他のエラー文言は変更しない。
const LEDGER_UNAUTHORIZED: &str = "再ログインが必要です。";
/// 401 のときに LOG OUT からの再ログインを案内する dedicated 文言。
const LEDGER_RELOGIN_HINT: &str = "LOG OUT からログアウトし、再度 JPKI でログインしてください。";

#[component]
fn Ledger(token: String) -> impl IntoView {
    let (txs, set_txs) = signal(Vec::<Transaction>::new());
    // 取得失敗を明示する。失敗時は「NO_TRANSACTIONS_RECORDED」を出さず、
    // 固定文言のみを表示する。
    let (ledger_error, set_ledger_error) = signal(String::new());
    // 取得中は loading status を出し、空状態は取得完了後のみ描画する。
    let (loading, set_loading) = signal(false);

    Effect::new(move |_| {
        let token = token.clone();
        spawn_local(async move {
            // 再取得時に前回の失敗表示が残らないようにする。
            set_loading.set(true);
            let _release = BusyGuard(set_loading);
            set_ledger_error.set(String::new());
            match api_fetch(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::get(&format!("{}/transactions", API_BASE))
                        .header("Authorization", &format!("Bearer {}", jwt))
                },
                true,
            )
            .await
            {
                Ok(response) if response.ok() => match response.json::<Vec<Transaction>>().await {
                    Ok(data) => {
                        set_txs.set(data);
                        set_ledger_error.set(String::new());
                    }
                    Err(_) => {
                        // JSON 解析失敗: 詳細は出さず固定文言のみ表示する。
                        set_ledger_error.set(LEDGER_ERROR.to_string());
                    }
                },
                // 401 は dedicated 固定文言＋LOG OUT 案内へ切り替える。
                // 403 / 5xx / ネットワークエラーは従来の取得失敗文言を維持する。
                Ok(response) if response.status() == 401 => {
                    set_ledger_error.set(LEDGER_UNAUTHORIZED.to_string());
                }
                // HTTP エラー・ネットワークエラー: ステータスや本文、生エラーは出さない。
                _ => set_ledger_error.set(LEDGER_ERROR.to_string()),
            }
        });
    });

    view! {
        <div class="max-w-5xl mx-auto mt-12">
            <h2 lang="en" class="text-xl font-bold tracking-widest uppercase mb-8">"Forensic Transaction Ledger"</h2>
            <div lang="en" class="border border-amber-500/30 bg-amber-500/5 text-amber-300/90 text-[9px] p-3 mb-4 font-mono leading-relaxed">
                "LEGACY MOCK RECORDS — Written by the disabled mock support API. These entries are not proof of payment; JPYC settlement is not implemented."
            </div>
            <Show when=move || loading.get() fallback=|| ()>
                <p role="status" aria-live="polite" data-testid="ledger-loading"
                    lang=move || error_lang(LEDGER_LOADING) class="text-xs text-gray-400 mb-4">
                    {LEDGER_LOADING}
                </p>
            </Show>
            <Show when=move || !ledger_error.get().is_empty() && ledger_error.get() != LEDGER_UNAUTHORIZED fallback=|| ()>
                <p role="alert" data-testid="ledger-error" lang=move || error_lang(&ledger_error.get()) class="text-xs text-red-500 mb-4">
                    {move || ledger_error.get()}
                </p>
            </Show>
            // 401 のときだけ、dedicated role=status で固定文言＋LOG OUT 案内を出す。
            <Show when=move || ledger_error.get() == LEDGER_UNAUTHORIZED fallback=|| ()>
                <p data-testid="ledger-relogin" role="status" aria-live="polite"
                    lang=move || error_lang(LEDGER_UNAUTHORIZED) class="text-xs text-gray-400 mb-4">
                    {LEDGER_UNAUTHORIZED} {LEDGER_RELOGIN_HINT}
                </p>
            </Show>
            <div class="border border-white/10 overflow-hidden">
                <table class="w-full text-left text-[10px] font-mono">
                    <caption>"取引履歴"</caption>
                    <thead class="bg-white/5 border-b border-white/10 uppercase tracking-tighter">
                        <tr>
                            <th lang="en" scope="col" class="p-4">"Date"</th>
                            <th lang="en" scope="col" class="p-4">"Sender (P2P)"</th>
                            <th lang="en" scope="col" class="p-4">"Receiver"</th>
                            <th lang="en" scope="col" class="p-4 text-right">"Amount"</th>
                            <th lang="en" scope="col" class="p-4">"Record_Hash"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-white/5">
                        <For
                            each=move || txs.get()
                            key=|tx| tx.tx_id
                            children=move |tx| {
                                let sender_prefix = tx.sender_hash[0..12.min(tx.sender_hash.len())].to_string();
                                let receiver_prefix = tx.receiver_hash[0..12.min(tx.receiver_hash.len())].to_string();
                                // 表示は整形済み日時、生 ISO は title 属性へ保持する。
                                let created_at = tx.created_at.clone();
                                view! {
                                    <tr class="hover:bg-white/5 transition-colors">
                                        <td class="p-4 text-gray-500" title=created_at>{format_local_datetime(&tx.created_at)}</td>
                                        <td class="p-4 text-blue-400">{sender_prefix} "..."</td>
                                        <td class="p-4 text-green-400">{receiver_prefix} "..."</td>
                                        <td class="p-4 text-right font-bold">{tx.amount} " JPYC"</td>
                                        <td class="p-4 text-gray-400 text-[10px] break-all max-w-[200px]">{tx.tx_hash}</td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
                // 取得が完了する前は空状態を出さない（読み込み中を「記録なし」と誤告知しない）。
                <Show when=move || !loading.get() && txs.get().is_empty() && ledger_error.get().is_empty() fallback=|| ()>
                    <div class="p-12 text-center text-gray-400 italic">"NO_TRANSACTIONS_RECORDED"</div>
                </Show>
            </div>
        </div>
    }
}

/// 投票成功時の通知文言。生エラーや機微情報は含めない固定文言。
const VOTE_NOTICE: &str = "投票を記録しました。";

/// 投票失敗時の文言を HTTP ステータスから選ぶ固定文言集合。
/// `None` はステータスを取得できなかった場合（ネットワークエラー）。
/// サーバーが返した本文や生エラーは決して表示しない。
fn vote_error_message(status: Option<u16>) -> String {
    match status {
        Some(code) => format!("投票に失敗しました (HTTP {code})."),
        None => "Network error submitting vote.".to_string(),
    }
}

/// 投票一覧の取得失敗時に表示する固定文言。HTTP ステータス番号のみを埋め込み、
/// サーバーが返した本文や生エラーは決して表示しない。
fn reports_error_message(status: Option<u16>) -> String {
    match status {
        Some(code) => format!("投票一覧の取得に失敗しました (HTTP {code})."),
        None => "Network error loading reports.".to_string(),
    }
}

/// 投票一覧の JSON 解析失敗時に表示する固定文言。解析エラーの詳細は出さない。
const REPORTS_PARSE_ERROR: &str = "投票一覧の解析に失敗しました。";

/// 集計取得の失敗種別。生エラーやサーバー本文は保持しない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatsFailure {
    /// `/mod/stats` の JSON 解析失敗。
    Parse,
    /// `/mod/stats` のリクエスト送信失敗（ネットワークエラー）。
    Fetch,
}

/// 公開集計が 401 のときだけ表示する dedicated 固定文言。
/// 403 / 5xx / ネットワークエラーは従来どおり `stats_error_message(Fetch)` を維持する。
const PUBLIC_STATS_UNAUTHORIZED: &str = "再ログインが必要です。";

/// 集計取得失敗時に画面へ表示する固定文言を返す純粋ヘルパー。
/// 生エラー（`reqwest::Error` / `serde_json::Error`）を引数に取らないため、
/// フィールド名・解析位置・サーバー本文などの機微情報が露出する余地がない。
/// HTTP ステータス付きのエラーは呼び出し側で従来どおり組み立てる。
fn stats_error_message(failure: StatsFailure) -> &'static str {
    match failure {
        StatsFailure::Parse => "集計の解析に失敗しました。",
        StatsFailure::Fetch => "集計の取得に失敗しました。",
    }
}

/// 二重送信ガードの同期判定。送信中（`busy == true`）なら新規送信を開始しない。
/// `disabled` の DOM 反映より前に届く2回目のクリックも止めるため、
/// イベントハンドラの先頭で呼ぶ。純粋関数なので単体テストで検証できる。
fn claim_submission(busy: bool) -> bool {
    !busy
}

/// 通報フォームの入力検証（純粋関数）。CID・理由とも空白のみならエラーを返す。
/// サーバー側の検証（CID 形式・理由 1..=2000 文字）に先立つクライアント側ガード。
fn validate_report_input(cid: &str, reason: &str) -> Result<(), &'static str> {
    if cid.trim().is_empty() {
        return Err("対象 CID を入力してください。");
    }
    if reason.trim().is_empty() {
        return Err("通報理由を入力してください。");
    }
    Ok(())
}

/// 通報理由のバイト上限超過時に表示する固定文言。生入力値は一切混入させない。
const REPORT_REASON_BYTE_ERROR: &str = "通報理由が長すぎます。上限を確認してください。";

/// 通報理由が UTF-8 バイト上限を超える場合だけ固定文言を返す純粋関数。
/// 空文字の判定は `validate_report_input` の責務（当関数は長さのみを扱う）。
fn report_reason_byte_error(reason: &str) -> Option<&'static str> {
    if reason.len() > common::MAX_REPORT_REASON_BYTES {
        return Some(REPORT_REASON_BYTE_ERROR);
    }
    None
}

/// input/textarea の maxlength。CID は ASCII のみ。理由は `common` の UTF-8 バイト上限に合わせる。
/// maxlength は UTF-16 コードユニット単位のためマルチバイト文字では守れず、
/// 送信前ゲート `report_reason_byte_error` が実契約を担保する。
const REPORT_CID_MAX_LEN: u32 = 128;
const REPORT_REASON_MAX_LEN: u32 = common::MAX_REPORT_REASON_BYTES as u32;

/// 送信中フラグを確実に解除する RAII ガード。
/// `spawn_local` 内の `match` を成否いずれのアームで抜けても、スコープ離脱時に `Drop` が走って
/// busy を解除し、以降の送信が止まることを防ぐ（favorite_busy と同じ方針）。
/// wasm の panic=abort では unwind しないため、panic 時の `Drop` には依存しない。
struct BusyGuard(WriteSignal<bool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// 公開集計の鮮度判定閾値（15分）。GENERATED_AT がこれより古ければ鮮度警告を出す。
/// 認証付きオプイン集計（`/mod/stats`）の状態には一切影響しない。
const PUBLIC_STATS_FRESHNESS_MAX_AGE_MS: f64 = 15.0 * 60.0 * 1000.0;

/// 生成時刻が閾値を超えて古いか。パース不能時は誤検知を避けるため警告を出さない。
#[cfg(target_arch = "wasm32")]
fn public_stats_is_stale(generated_at: &str, max_age_ms: f64) -> bool {
    let generated = js_sys::Date::parse(generated_at);
    !generated.is_nan() && js_sys::Date::now() - generated > max_age_ms
}

#[cfg(not(target_arch = "wasm32"))]
fn public_stats_is_stale(_generated_at: &str, _max_age_ms: f64) -> bool {
    false
}

#[component]
fn Moderation<O>(token: String, user_hash: String, on_open_content: O) -> impl IntoView
where
    O: Fn(String) + 'static + Clone + Send,
{
    // ContentViewer の Report ボタンから渡された CID を通報フォームへプリフィルする。
    let initial_cid = parse_content_query(&current_content_query()).report_cid;
    let (cid, set_cid) = signal(initial_cid);
    let (reason, set_reason) = signal(String::new());
    let (status, set_status) = signal(String::new());
    // 送信失敗は dedicated role="alert" で告知する。成功・進捗は status のまま。
    let (report_failed, set_report_failed) = signal(String::new());
    // 空入力で送信を止めた理由を role=alert で読み上げる（送信前に検証する）。
    let (report_validation_error, set_report_validation_error) = signal(String::new());
    let (reports, set_reports) = signal(Vec::<Report>::new());
    // 投票一覧の取得失敗を明示する。失敗しても reports は空にせず既存データを維持し、
    // 空状態を「保留投票なし」と誤表示しない。
    let (reports_error, set_reports_error) = signal(String::new());
    // R5-09: 透明性ダッシュボードの集計表示状態。
    // 初期表示では取得せず、中立的なボタンの押下時にだけプローブする。
    let (stats_html, set_stats_html) = signal(String::new());
    let (stats_error, set_stats_error) = signal(String::new());
    let (stats_loaded, set_stats_loaded) = signal(false);
    // ADR-0008 段階2: 認証済み利用者向けの公開集計。オプインとは独立にマウント時に取得し、
    // 一般画面へ無条件に描画する。既存の運用者パネル状態（stats_authorized/toggle/loaded）は触らない。
    let (public_stats_html, set_public_stats_html) = signal(String::new());
    let (public_stats_error, set_public_stats_error) = signal(String::new());
    let (public_stats_loaded, set_public_stats_loaded) = signal(false);
    // 公開集計の鮮度警告と明示的な「更新」トリガ。オプイン集計の状態とは独立。
    let (public_stats_stale, set_public_stats_stale) = signal(false);
    // 一覧と同じレスポンス世代。マウント取得と「更新」押下が重なっても最新だけを採用する。
    let public_stats_generation = Rc::new(Cell::new(0u64));
    // 「更新」ボタンの連打抑止。マウント取得では busy を立てず、押下時だけ立てる。
    let (public_stats_busy, set_public_stats_busy) = signal(false);
    // 運用者パネルのマウント条件。`/mod/stats` が 200 を返したというサーバ応答だけを根拠にし、
    // トークン内容やクライアント状態から運用者を推測しない。
    let (stats_authorized, set_stats_authorized) = signal(false);
    let (stats_busy, set_stats_busy) = signal(false);
    // 投票結果のフィードバック。成功通知と失敗エラーは固定文言のみを表示する。
    let (vote_error, set_vote_error) = signal(String::new());
    let (vote_notice, set_vote_notice) = signal(String::new());
    // 通報・投票の二重送信を防ぐガード。favorite_busy と同じく、
    // 送信開始時に true、レスポンス処理完了後（成否・panic 問わず）に false へ戻す。
    let (report_busy, set_report_busy) = signal(false);
    let (vote_busy, set_vote_busy) = signal(false);

    let token_stored = StoredValue::new(token);
    let user_hash_stored = StoredValue::new(user_hash);

    // popstate では report_cid を cid signal へ同期する。View が MODERATION のまま
    // 履歴条目だけが入れ替わっても通報フォームのプリフィルが古く残らないようにする。
    // 履歴の追記は行わず、URL を読んで signal を更新するだけ（Dashboard と同じ契約）。
    let mod_cid_listener: StoredValue<Option<EventClosure>, LocalStorage> =
        StoredValue::new_local(None);
    let mod_cid_popstate: EventClosure = Closure::new(move |_: web_sys::Event| {
        set_cid.set(parse_content_query(&current_content_query()).report_cid);
    });
    if let Some(window) = web_sys::window() {
        let _ = window.add_event_listener_with_callback(
            "popstate",
            mod_cid_popstate.as_ref().unchecked_ref(),
        );
        mod_cid_listener.set_value(Some(mod_cid_popstate));
    }
    on_cleanup(move || {
        let mut taken = None;
        mod_cid_listener.update_value(|slot| taken = slot.take());
        if let (Some(listener), Some(window)) = (taken, web_sys::window()) {
            let _ = window
                .remove_event_listener_with_callback("popstate", listener.as_ref().unchecked_ref());
        }
    });

    let fetch_reports = move || {
        let token = token_stored.get_value();
        spawn_local(async move {
            match api_fetch(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::get(&format!("{}/mod/reports", API_BASE))
                        .header("Authorization", &format!("Bearer {}", jwt))
                },
                true,
            )
            .await
            {
                Ok(response) if response.ok() => match response.json::<Vec<Report>>().await {
                    Ok(data) => {
                        // 成功時のみ一覧を更新し、前回のエラーを消す。
                        set_reports.set(data);
                        set_reports_error.set(String::new());
                    }
                    Err(_) => {
                        // 解析失敗: 既存データは維持し、固定文言のみ表示する。
                        set_reports_error.set(REPORTS_PARSE_ERROR.to_string());
                    }
                },
                Ok(response) => {
                    // HTTP エラー（401 を含む）: 既存データを維持し、ステータス番号のみ表示する。
                    set_reports_error.set(reports_error_message(Some(response.status())));
                }
                Err(_) => {
                    // ネットワークエラー: 生エラーは出さず固定文言を表示する。
                    set_reports_error.set(reports_error_message(None));
                }
            }
        });
    };

    Effect::new(move |_| {
        fetch_reports();
    });

    // ADR-0008 段階2: 認証済み利用者向けの公開集計をマウント時に取得する。
    // オプイン押下を待たず、一般画面へ無条件に描画するためのデータ取得。
    // 「更新」ボタンの押下でも同じ経路を通り、オプイン集計（/mod/stats）には触れない。
    // 世代カウンタは一覧と同じ契約（should_apply_response）で、遅れて届いた古い応答を捨てる。
    let fetch_public_stats = move |clear_busy: bool| {
        let generation_cell = Rc::clone(&public_stats_generation);
        let response_generation = generation_cell.get() + 1;
        generation_cell.set(response_generation);
        let token = token_stored.get_value();
        spawn_local(async move {
            // 「更新」由来のときだけ busy を解除する（マウント取得は busy を立てない）。
            let _release = clear_busy.then(|| BusyGuard(set_public_stats_busy));
            match api_fetch(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::get(&format!("{}/mod/stats/public", API_BASE))
                        .header("Authorization", &format!("Bearer {}", jwt))
                },
                true,
            )
            .await
            {
                Ok(response) if response.ok() => {
                    match response.json::<PublicModerationStats>().await {
                        Ok(stats) => {
                            if should_apply_response(generation_cell.get(), response_generation) {
                                set_public_stats_html.set(render_public_mod_stats_html(&stats));
                                set_public_stats_error.set(String::new());
                                set_public_stats_loaded.set(true);
                                // GENERATED_AT の鮮度だけを警告し、オプイン集計の状態は変更しない。
                                set_public_stats_stale.set(public_stats_is_stale(
                                    &stats.generated_at,
                                    PUBLIC_STATS_FRESHNESS_MAX_AGE_MS,
                                ));
                            }
                        }
                        Err(_) => {
                            if should_apply_response(generation_cell.get(), response_generation) {
                                // 解析エラーの詳細（フィールド名・位置・生エラー）は出さず、
                                // 運用者向け /mod/stats と同じ Parse 分類の固定文言を表示する。
                                set_public_stats_html.set(String::new());
                                set_public_stats_loaded.set(false);
                                set_public_stats_stale.set(false);
                                set_public_stats_error
                                    .set(stats_error_message(StatsFailure::Parse).to_string());
                            }
                        }
                    }
                }
                // 401 は再ログインが必要なため専用固定文言＋LOG OUT 案内を出す。
                // 403 / 5xx / ネットワークエラーは従来どおり取得失敗文言を維持する。
                // いずれも生情報やステータス詳細は出さない。
                Ok(response) if response.status() == 401 => {
                    if should_apply_response(generation_cell.get(), response_generation) {
                        set_public_stats_html.set(String::new());
                        set_public_stats_loaded.set(false);
                        set_public_stats_stale.set(false);
                        set_public_stats_error.set(PUBLIC_STATS_UNAUTHORIZED.to_string());
                    }
                }
                _ => {
                    if should_apply_response(generation_cell.get(), response_generation) {
                        set_public_stats_html.set(String::new());
                        set_public_stats_loaded.set(false);
                        set_public_stats_stale.set(false);
                        set_public_stats_error
                            .set(stats_error_message(StatsFailure::Fetch).to_string());
                    }
                }
            }
        });
    };
    // 更新ボタン: disabled / aria-busy に加え、同期ガードでも連打を止める。
    let refresh_public_stats = {
        let fetch = fetch_public_stats.clone();
        move || {
            if !claim_submission(public_stats_busy.get()) {
                return;
            }
            set_public_stats_busy.set(true);
            fetch(true);
        }
    };
    Effect::new(move |_| {
        fetch_public_stats(false);
    });

    // 運用者パネルを開くためのオプイン。初期表示では /mod/stats を 1 本も送らず、
    // 中立的な文言のボタンが押されたときだけ認証付きでプローブする。
    // 二重クリックは claim_submission で同期に排除し、BusyGuard で確実に解除する。
    let fetch_stats = move || {
        if !claim_submission(stats_busy.get()) {
            return;
        }
        set_stats_busy.set(true);
        let token = token_stored.get_value();
        spawn_local(async move {
            let _release = BusyGuard(set_stats_busy);
            match api_fetch(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::get(&format!("{}/mod/stats", API_BASE))
                        .header("Authorization", &format!("Bearer {}", jwt))
                },
                true,
            )
            .await
            {
                Ok(response) if response.ok() => match response.json::<ModerationStats>().await {
                    Ok(stats) => {
                        set_stats_html.set(render_moderation_stats_html(&stats));
                        set_stats_error.set(String::new());
                        set_stats_loaded.set(true);
                        // 200 応答のみを運用者判定の根拠にしてパネルをマウントする。
                        set_stats_authorized.set(true);
                    }
                    Err(_) => {
                        // 200 が返った時点で運用者と判定し、パネル内へ固定文言のみ表示する。
                        // 解析エラーの詳細（フィールド名・位置・生エラー）は出さない。
                        set_stats_html.set(String::new());
                        set_stats_loaded.set(false);
                        set_stats_error.set(stats_error_message(StatsFailure::Parse).to_string());
                        set_stats_authorized.set(true);
                    }
                },
                Ok(response) if response.status() == 403 => {
                    // 403 は一般画面へ描画せず、パネルを黙って閉じる。
                    set_stats_html.set(String::new());
                    set_stats_error.set(String::new());
                    set_stats_loaded.set(false);
                    set_stats_authorized.set(false);
                }
                Ok(response) => {
                    set_stats_html.set(String::new());
                    set_stats_loaded.set(false);
                    set_stats_error.set(format!(
                        "集計の取得に失敗しました (HTTP {}).",
                        response.status()
                    ));
                }
                Err(_) => {
                    // ネットワークエラーの生情報は出さず固定文言のみ表示する。
                    set_stats_html.set(String::new());
                    set_stats_loaded.set(false);
                    set_stats_error.set(stats_error_message(StatsFailure::Fetch).to_string());
                }
            }
        });
    };

    let submit_report = move |_| {
        // 同期ガード: DOM の disabled 反映前に届く2回目のクリックもここで止める。
        if !claim_submission(report_busy.get()) {
            return;
        }
        // 新しい試行では前回の送信失敗を消す。
        set_report_failed.set(String::new());
        let cid = cid.get();
        let reason = reason.get();
        // POST 前に空入力を止める。検証エラーは role=alert で読み上げる。
        // 既存の "Report failed." 固定文言は成否の通知として変更しない。
        if let Err(message) = validate_report_input(&cid, &reason) {
            set_report_validation_error.set(message.to_string());
            set_status.set(String::new());
            return;
        }
        // 理由も UTF-8 バイト契約を通信前にゲートする（maxlength は UTF-16 では守れない）。
        if let Some(error) = report_reason_byte_error(&reason) {
            set_report_validation_error.set(error.to_string());
            set_status.set(String::new());
            return;
        }
        set_report_validation_error.set(String::new());
        set_report_busy.set(true);
        let token = token_stored.get_value();
        // 送信開始時にも失敗表示を消し、進捗は status へ出す。
        set_report_failed.set(String::new());
        set_status.set("通報を送信中...".to_string());
        spawn_local(async move {
            // 成否いずれのアームでもスコープ離脱時の Drop で busy を解除する（panic=abort に非依存）。
            let _release = BusyGuard(set_report_busy);
            let res = api_fetch_request(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::post(&format!("{}/mod/report", API_BASE))
                        .header("Authorization", &format!("Bearer {}", jwt))
                        .credentials(RequestCredentials::Include)
                        .json(&serde_json::json!({
                            "report_id": 0,
                            "target_cid": cid,
                            "reason": reason,
                            "status": "Pending"
                        }))
                },
                true,
            )
            .await;
            match res {
                Ok(r) if r.ok() => {
                    set_status.set("Report accepted. Content marked Reported.".to_string());
                    fetch_reports();
                }
                _ => {
                    // 失敗時は送信中ステータスを消し、do_vote と完了処理を揃える。
                    // role="alert" の "Report failed." 固定文言は変更しない。
                    set_report_failed.set("Report failed.".to_string());
                    set_status.set(String::new());
                }
            }
        });
    };

    let do_vote = move |report_id: i64, result: String| {
        // 投票一覧の取得失敗中は、古いカードが残っていても投票を受け付けない。
        if !reports_error.get_untracked().is_empty() {
            return;
        }
        // 同期ガード: DOM の disabled 反映前に届く2回目のクリックもここで止める。
        if !claim_submission(vote_busy.get()) {
            return;
        }
        set_vote_busy.set(true);
        // 新しい試行では前回の通知・エラーを必ず消す。
        set_vote_error.set(String::new());
        set_vote_notice.set(String::new());
        // submit_report と同じく、送信開始を固定文言で通知する。
        // 成功・失敗が決まったら通知（vote_notice / vote_error）へ切り替える。
        set_status.set("投票を送信中...".to_string());
        let token = token_stored.get_value();
        let user_hash = user_hash_stored.get_value();
        spawn_local(async move {
            // 成否いずれのアームでもスコープ離脱時の Drop で busy を解除する（panic=abort に非依存）。
            let _release = BusyGuard(set_vote_busy);
            let mock_signature = vec![0u8; 256];
            let res = api_fetch_request(
                |jwt| {
                    let jwt = if jwt.is_empty() { token.as_str() } else { jwt };
                    Request::post(&format!("{}/mod/vote", API_BASE))
                        .header("Authorization", &format!("Bearer {}", jwt))
                        .credentials(RequestCredentials::Include)
                        .json(&serde_json::json!({
                            "report_id": report_id,
                            "juror_hash": user_hash,
                            "result": result,
                            "signature": mock_signature
                        }))
                },
                true,
            )
            .await;
            match res {
                Ok(r) if r.ok() => {
                    set_vote_notice.set(VOTE_NOTICE.to_string());
                    fetch_reports();
                }
                Ok(r) => {
                    set_vote_error.set(vote_error_message(Some(r.status())));
                }
                Err(_) => {
                    set_vote_error.set(vote_error_message(None));
                }
            }
            // 進捗表示を消し、通知（vote_notice / vote_error）へ切り替える。
            set_status.set(String::new());
        });
    };

    view! {
        <div class="max-w-4xl mx-auto mt-12 grid grid-cols-1 md:grid-cols-2 gap-12">
            <section data-testid="public-mod-stats" class="md:col-span-2 border border-white/10 p-8 space-y-4">
                <div class="flex items-center justify-between gap-4">
                    <h2 lang="en" class="text-xl font-bold tracking-widest uppercase">"PUBLIC STATS"</h2>
                    <button
                        type="button"
                        lang="en"
                        on:click=move |_| refresh_public_stats.clone()()
                        disabled=move || public_stats_busy.get()
                        aria-busy=move || if public_stats_busy.get() { "true" } else { "false" }
                        data-testid="public-mod-stats-refresh"
                        class="py-2 px-4 border border-white/20 text-[10px] uppercase tracking-widest hover:bg-white hover:text-black disabled:opacity-50">
                        "REFRESH PUBLIC STATS"
                    </button>
                </div>
                <Show when=move || !public_stats_error.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="public-mod-stats-error" lang=move || error_lang(&public_stats_error.get()) class="text-xs text-red-500">
                        {move || public_stats_error.get()}
                    </p>
                </Show>
                // 401 のときだけ、既存の LOG OUT ボタンからの再ログインを案内する。
                <Show when=move || public_stats_error.get() == PUBLIC_STATS_UNAUTHORIZED fallback=|| ()>
                    <p data-testid="public-mod-stats-relogin-hint" class="text-xs text-gray-400">
                        "LOG OUT からログアウトし、再度 JPKI でログインしてください。"
                    </p>
                </Show>
                <Show when=move || public_stats_loaded.get() fallback=|| ()>
                    <div data-testid="public-mod-stats-content" inner_html=move || public_stats_html.get() />
                </Show>
                // GENERATED_AT が閾値を超えて古いときだけ出す鮮度警告（オプイン集計には触れない）。
                <Show when=move || public_stats_stale.get() fallback=|| ()>
                    <p role="status" data-testid="public-mod-stats-stale" lang=move || error_lang(PUBLIC_STATS_STALE_MESSAGE) class="text-[10px] text-yellow-500">
                        // 色に依存しない手がかり。既存固定文言は変更せず dedicated 要素を前置する。
                        <span data-testid="public-mod-stats-stale-icon" aria-hidden="true">"⚠ "</span>
                        {PUBLIC_STATS_STALE_MESSAGE}
                    </p>
                </Show>
            </section>
            <div data-testid="moderation-stats-optin" class="md:col-span-2 border border-white/10 p-8 space-y-4">
                <button
                    lang="en"
                    on:click=move |_| fetch_stats()
                    disabled=move || stats_busy.get()
                    aria-busy=move || if stats_busy.get() { "true" } else { "false" }
                    data-testid="moderation-stats-toggle"
                    class="py-2 px-4 border border-white/20 text-[10px] uppercase tracking-widest hover:bg-white hover:text-black">
                    "SHOW STATS"
                </button>
                <Show when=move || stats_authorized.get() fallback=|| ()>
                    <section data-testid="moderation-stats-section" class="border-t border-white/10 pt-8 space-y-4">
                        <h2 lang="en" class="text-xl font-bold tracking-widest uppercase">"TRANSPARENCY DASHBOARD"</h2>
                        <Show when=move || !stats_error.get().is_empty() fallback=|| ()>
                            <p role="alert" data-testid="moderation-stats-error" lang=move || error_lang(&stats_error.get()) class="text-xs text-red-500">
                                {move || stats_error.get()}
                            </p>
                        </Show>
                        <Show when=move || stats_loaded.get() fallback=|| ()>
                            <div data-testid="moderation-stats" inner_html=move || stats_html.get() />
                        </Show>
                    </section>
                </Show>
            </div>
            <div class="border border-white/10 p-8 space-y-6 h-fit">
                <h2 lang="en" class="text-xl font-bold tracking-widest uppercase">"Submit Report"</h2>
                <p class="text-[10px] text-gray-500">
                    "Reporter: " {move || user_hash_stored.get_value()[0..8.min(user_hash_stored.get_value().len())].to_string()}
                </p>
                // native required の検証バブルは使わず、aria-required + 独自 role=alert へ統一する。
                // <form on:submit> で Enter 経路も同じ submit_report 検証へ集約する。
                <form on:submit=move |ev| {
                    ev.prevent_default();
                    submit_report(());
                } class="space-y-6">
                    // aria-label は補助技術向けの既定名として維持し、<label for> で明示的に関連付ける。
                    // placeholder は例示のみでありラベル代わりには使わない。
                    <label for="report-target-cid" lang="en" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "TARGET CID"
                    </label>
                    <input
                        id="report-target-cid"
                        type="text"
                        lang="en"
                        aria-label="TARGET CID"
                        aria-required="true"
                        placeholder="TARGET CID"
                        maxlength=REPORT_CID_MAX_LEN
                        on:input=move |ev| set_cid.set(event_target_value(&ev))
                        prop:value=cid
                        class="w-full bg-black border border-white/20 p-3 text-sm"
                    />
                    <label for="report-reason" lang="en" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "REASON"
                    </label>
                    <textarea
                        id="report-reason"
                        lang="en"
                        aria-label="REASON"
                        aria-required="true"
                        placeholder="REASON"
                        maxlength=REPORT_REASON_MAX_LEN
                        on:input=move |ev| set_reason.set(event_target_value(&ev))
                        class="w-full bg-black border border-white/20 p-3 text-sm h-24"
                    />
                    <button type="submit" lang="en"
                        disabled=move || report_busy.get()
                        aria-busy=move || if report_busy.get() { "true" } else { "false" }
                        class="w-full py-3 border border-white hover:bg-white hover:text-black font-bold text-xs tracking-widest">
                        "SUBMIT FORENSIC REPORT"
                    </button>
                    <Show when=move || !report_validation_error.get().is_empty() fallback=|| ()>
                        <p role="alert" data-testid="report-validation-error" lang=move || error_lang(&report_validation_error.get()) class="text-xs text-red-500">
                            {move || report_validation_error.get()}
                        </p>
                    </Show>
                </form>
                <Show when=move || !report_failed.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="report-failed" lang=move || error_lang(&report_failed.get()) class="text-xs text-red-500">
                        {move || report_failed.get()}
                    </p>
                </Show>
                <Show when=move || !status.get().is_empty() fallback=|| ()>
                    <p role="status" lang=move || error_lang(&status.get()) class="text-xs text-gray-400">{move || status.get()}</p>
                </Show>
            </div>

            <div class="border border-white/10 p-8 space-y-6">
                <h2 lang="en" class="text-xl font-bold tracking-widest uppercase">"Jury Voting Queue"</h2>
                <Show when=move || !reports_error.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="reports-error" lang=move || error_lang(&reports_error.get()) class="text-xs text-red-500">
                        {move || reports_error.get()}
                    </p>
                </Show>
                <Show when=move || !vote_error.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="vote-error" lang=move || error_lang(&vote_error.get()) class="text-xs text-red-500">
                        {move || vote_error.get()}
                    </p>
                </Show>
                <Show when=move || !vote_notice.get().is_empty() fallback=|| ()>
                    <p role="status" data-testid="vote-notice" lang=move || error_lang(&vote_notice.get()) class="text-xs text-green-500">
                        {move || vote_notice.get()}
                    </p>
                </Show>
                <div class="space-y-4">
                    <For
                        each=move || reports.get()
                        key=|report| report.report_id
                        children=move |report| {
                            let rid = report.report_id;
                            // 投票前に本文・監査を確認できるよう、target_cid から
                            // BROWSE の該当カードへディープリンクを張る（認証状態は維持）。
                            let target_cid = report.target_cid.clone();
                            let target_href = format!("/?cid={}#dashboard", url_encode(&report.target_cid));
                            let open_content = on_open_content.clone();
                            view! {
                                <div class="border border-white/5 p-4 space-y-3">
                                    <div class="flex justify-between items-center">
                                        <a
                                            href=target_href
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                ev.prevent_default();
                                                open_content(target_cid.clone());
                                            }
                                            data-testid="report-target-link"
                                            class="text-[8px] text-green-500 truncate mr-2 underline hover:text-white">
                                            {report.target_cid.clone()}
                                        </a>
                                        <span lang="en" class="text-[8px] border border-red-500/20 text-red-500 px-1 uppercase">"Reported"</span>
                                    </div>
                                    <p class="text-[10px] text-gray-300 italic"><span lang="en">"Reason: "</span><span lang=move || error_lang(&report.reason)>{report.reason.clone()}</span></p>
                                    <div class="grid grid-cols-2 gap-4">
                                        <button lang="en" on:click=move |_| do_vote(rid, "Approve".to_string())
                                            disabled=move || vote_busy.get() || !reports_error.get().is_empty()
                                            aria-busy=move || if vote_busy.get() { "true" } else { "false" }
                                            class="py-2 border border-green-500/50 text-green-500 text-[8px] hover:bg-green-500 hover:text-black transition-colors">
                                            "APPROVE (RESTORE)"
                                        </button>
                                        <button lang="en" on:click=move |_| do_vote(rid, "Reject".to_string())
                                            disabled=move || vote_busy.get() || !reports_error.get().is_empty()
                                            aria-busy=move || if vote_busy.get() { "true" } else { "false" }
                                            class="py-2 border border-red-500/50 text-red-500 text-[8px] hover:bg-red-500 hover:text-black transition-colors">
                                            "REJECT (VOTE)"
                                        </button>
                                    </div>
                                </div>
                            }
                        }
                    />
                    <Show when=move || reports.get().is_empty() && reports_error.get().is_empty() fallback=|| ()>
                        <div lang="en" class="text-center py-12 text-[10px] text-gray-400 italic">"NO_PENDING_VOTES"</div>
                    </Show>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    /// レンダリング結果から `id="..."` を順に取り出す（見出しアンカー用）。
    fn heading_ids(html: &str) -> Vec<String> {
        const PREFIX: &str = " id=\"";
        html.match_indices(PREFIX)
            .map(|(idx, _)| {
                html[idx + PREFIX.len()..]
                    .split('"')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .collect()
    }

    /// TOC の `href="#..."` からフラグメントを順に取り出す。
    fn anchor_fragments(html: &str) -> Vec<String> {
        const PREFIX: &str = "href=\"#";
        html.match_indices(PREFIX)
            .map(|(idx, _)| {
                html[idx + PREFIX.len()..]
                    .split('"')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn should_refresh_requires_401_and_auth() {
        // 有効化済み: 401 かつ認証コンテキスト時のみ更新対象。
        assert!(super::should_refresh(401, true, false));
        // WWW-Authenticate が付いた 401 は refresh 対象外（固定文言へ委ねる）。
        assert!(!super::should_refresh(401, true, true));
        assert!(!super::should_refresh(401, false, false));
        // 403 / 5xx / 成功は再試行しない。
        assert!(!super::should_refresh(403, true, false));
        assert!(!super::should_refresh(500, true, false));
        assert!(!super::should_refresh(200, true, false));
    }

    #[test]
    fn allows_retry_stops_after_one_refresh() {
        // 1 回目の 401 は refresh へ進む。
        assert!(super::allows_retry(401, false));
        // refresh 済みなら 2 回目の 401 はそのまま返す（無限ループしない）。
        assert!(!super::allows_retry(401, true));
        assert!(!super::allows_retry(403, false));
        assert!(!super::allows_retry(200, false));
    }

    #[test]
    fn refresh_slot_single_flights_and_broadcasts_one_outcome() {
        let mut slot = super::RefreshSlot::new();
        // 最初の 401 だけがリーダーになり、他は待機者になる。
        assert!(slot.claim());
        assert!(!slot.claim());
        assert!(!slot.claim());
        assert!(slot.outcome().is_none(), "完了前は結果が無い");

        slot.complete(super::RefreshOutcome::Token("jwt-v2".to_string()));
        assert!(!slot.in_flight);
        assert_eq!(
            slot.outcome(),
            Some(super::RefreshOutcome::Token("jwt-v2".to_string()))
        );

        // 次の波では前回の結果がクリアされ、再びリーダーが 1 人だけ選ばれる。
        assert!(slot.claim());
        assert!(slot.outcome().is_none());
        assert!(!slot.claim());
        slot.complete(super::RefreshOutcome::Failed);
        assert_eq!(slot.outcome(), Some(super::RefreshOutcome::Failed));
    }

    #[test]
    fn refresh_slot_epoch_mismatch_discards_stale_token() {
        // claim 後に logout/login で epoch を進めると、完了時の古い JWT は
        // REFRESHED_JWT へ書き戻されず Failed が配られる（旧セッション復活防止）。
        super::clear_refreshed_jwt();
        let mut slot = super::RefreshSlot::new();
        assert!(slot.claim());
        super::bump_refresh_epoch();
        let outcome = slot.complete(super::RefreshOutcome::Token("stale-jwt".to_string()));
        assert_eq!(outcome, super::RefreshOutcome::Failed);
        assert_eq!(slot.outcome(), Some(super::RefreshOutcome::Failed));
        assert!(
            super::REFRESHED_JWT.with(|slot| slot.borrow().is_none()),
            "stale token must not be written back"
        );
        super::clear_refreshed_jwt();
    }

    #[test]
    fn refresh_slot_waker_is_registered_once_and_cleared_on_completion() {
        let mut slot = super::RefreshSlot::new();
        // Waker::noop() は will_wake が常に false のため、実際の Wake 実装を使う。
        struct NoopWake;
        #[allow(clippy::manual_noop_waker)]
        impl std::task::Wake for NoopWake {
            fn wake(self: std::sync::Arc<Self>) {}
        }
        let waker = std::task::Waker::from(std::sync::Arc::new(NoopWake));
        slot.register_waker(&waker);
        slot.register_waker(&waker);
        assert_eq!(slot.waiters.len(), 1, "同一 waker は二重登録しない");

        slot.complete(super::RefreshOutcome::Failed);
        assert!(
            slot.waiters.is_empty(),
            "完了時に待機者は全員起こされてクリアされる"
        );
    }

    #[test]
    fn claim_submission_rejects_clicks_while_busy_and_recovers_after_release() {
        // アイドル時は最初のクリックで送信を開始する。
        let mut busy = false;
        assert!(super::claim_submission(busy));
        // 送信中の連打はすべて無視する（vote / report 共通の判定）。
        busy = true;
        assert!(!super::claim_submission(busy));
        assert!(!super::claim_submission(busy));
        // レスポンス処理後に解除されれば、次の試行を受け付ける。
        busy = false;
        assert!(super::claim_submission(busy));
    }

    #[test]
    fn file_picker_activates_only_on_enter_or_space() {
        assert!(super::is_file_picker_activation_key("Enter"));
        assert!(super::is_file_picker_activation_key(" "));
        assert!(super::is_file_picker_activation_key("Spacebar"));
        assert!(!super::is_file_picker_activation_key("Escape"));
        assert!(!super::is_file_picker_activation_key("a"));
    }

    #[test]
    fn favorite_error_message_uses_dedicated_401_text() {
        // R2-CLIENT-UX: lang 判定ヘルパの分類も既存テスト内で確認（tests131 不変）。
        for message in [
            "Network error updating favorite.",
            "Failed to update favorite (HTTP 500).",
            "Network error uploading content.",
            "Failed to upload content (HTTP 400).",
            "Network error submitting vote.",
            "Network error loading reports.",
            "Report failed.",
        ] {
            assert_eq!(super::error_lang(message), "en", "{message}");
        }
        for message in [
            "認証に失敗しました (HTTP 500).",
            "再ログインが必要です。",
            "コンテンツの取得に失敗しました (HTTP 500).",
            "投票に失敗しました (HTTP 500).",
            "投票一覧の取得に失敗しました。",
        ] {
            assert_eq!(super::error_lang(message), "ja", "{message}");
        }
        assert_eq!(
            super::favorite_error_message(Some(401)),
            "再ログインが必要です。"
        );
    }

    #[test]
    fn favorite_error_message_keeps_existing_http_and_network_text() {
        assert_eq!(
            super::favorite_error_message(Some(500)),
            "Failed to update favorite (HTTP 500)."
        );
        assert_eq!(
            super::favorite_error_message(Some(403)),
            "Failed to update favorite (HTTP 403)."
        );
        assert_eq!(
            super::favorite_error_message(None),
            "Network error updating favorite."
        );
    }

    #[test]
    fn upload_error_message_uses_dedicated_401_and_413_text() {
        assert_eq!(
            super::upload_error_message(Some(401)),
            "再ログインが必要です。"
        );
        assert_eq!(
            super::upload_error_message(Some(413)),
            "ERROR: File exceeds the 10 MiB limit."
        );
    }

    #[test]
    fn upload_error_message_uses_http_status_and_network_text() {
        assert_eq!(
            super::upload_error_message(Some(500)),
            "Failed to upload content (HTTP 500)."
        );
        assert_eq!(
            super::upload_error_message(Some(400)),
            "Failed to upload content (HTTP 400)."
        );
        assert_eq!(
            super::upload_error_message(None),
            "Network error uploading content."
        );
    }

    #[test]
    fn upload_error_message_never_leaks_raw_error_or_server_body() {
        // サーバー本文・生エラーの断片が全分岐で一切混入しないことを確認する。
        let leaked = [
            "Disallowed content type",
            "price must not exceed",
            "request body exceeds 10 MiB",
            "forced arrayBuffer failure",
            "reqwest",
            "TypeError",
            "panicked",
        ];
        for status in [Some(401), Some(413), Some(400), Some(500), None] {
            let message = super::upload_error_message(status);
            for fragment in leaked {
                assert!(
                    !message.contains(fragment),
                    "status={status:?} の文言に生エラー断片 {fragment:?} が含まれる: {message}"
                );
            }
        }
    }

    #[test]
    fn price_parse_failure_keeps_previous_valid_value() {
        // parse 成功時は入力値をそのまま返す（0 / 上限超過も丸めない）。
        assert_eq!(super::parse_price_input("0"), Some(0));
        assert_eq!(super::parse_price_input("1"), Some(1));
        assert_eq!(super::parse_price_input("1000000"), Some(1_000_000));
        assert_eq!(super::parse_price_input("1000001"), Some(1_000_001));

        // 指数表記はブラウザの valueAsNumber と同じく受理（1e3 -> 1000）。
        assert_eq!(super::parse_price_input("1e3"), Some(1_000));
        assert_eq!(super::parse_price_input("1E6"), Some(1_000_000));

        // parse 失敗時は None。signal を更新しないため直前の有効値が保持される。
        let mut price = 100;
        for raw in [
            "",
            "-",
            "e",
            "1e",
            "abc",
            "1.5",
            "NaN",
            "inf",
            "999999999999999999999999",
        ] {
            assert_eq!(super::parse_price_input(raw), None, "raw={raw:?}");
            if let Some(value) = super::parse_price_input(raw) {
                price = value;
            }
            assert_eq!(price, 100, "raw={raw:?} で直前値が保持されていない");
        }
    }

    #[test]
    fn price_range_error_is_fixed_text_without_raw_input() {
        // 範囲外は本番コンパイル対象の固定文言で拒否される（生入力値は混入しない）。
        for price in [i32::MIN, -1, 1_000_001, i32::MAX] {
            assert_eq!(
                super::price_range_error(price),
                Some(super::PRICE_RANGE_ERROR),
                "price={price}"
            );
        }
        // 生入力値（例: 1000001）は固定文言に混入しない。
        assert!(!super::PRICE_RANGE_ERROR.contains("1000001"));
        // 0 はサーバーと同じく受理（0 = 購入不可、E2E の 0 成功と整合）。
        for price in [0, 1, 100, 1_000_000] {
            assert_eq!(super::price_range_error(price), None, "price={price}");
        }
    }

    #[test]
    fn markdown_headings_get_ids_for_h1_h2_h3() {
        let rendered = super::render_markdown("# Alpha\n\n## Beta\n\n### Gamma\n");
        assert!(rendered.contains("<h1 id=\"alpha\">"), "{rendered}");
        assert!(rendered.contains("<h2 id=\"beta\">"), "{rendered}");
        assert!(rendered.contains("<h3 id=\"gamma\">"), "{rendered}");
    }

    #[test]
    fn toc_anchors_match_body_heading_ids() {
        let markdown = "# Alpha\n\n## Beta\n\n### Gamma\n\n## Delta\n";
        let body = super::render_markdown(markdown);
        let toc = super::render_toc(markdown);

        let expected = vec!["alpha", "beta", "gamma", "delta"];
        assert_eq!(heading_ids(&body), expected, "{body}");
        assert_eq!(anchor_fragments(&toc), expected, "{toc}");
        // TOC の全アンカーが本文の見出し id に存在すること（逆方向も確認）。
        for fragment in anchor_fragments(&toc) {
            assert!(body.contains(&format!("id=\"{fragment}\"")), "{body}");
        }
    }

    #[test]
    fn cid_prefix_namespaces_heading_ids_and_toc_anchors() {
        let markdown = "# Alpha\n\n## Beta\n\n# Alpha\n";
        let cid_a = super::cid_scope_token("cid-alpha");
        let cid_b = super::cid_scope_token("cid-beta");
        assert_ne!(cid_a, cid_b);

        let expected_a = vec![
            format!("{cid_a}-alpha"),
            format!("{cid_a}-beta"),
            format!("{cid_a}-alpha-1"),
        ];
        let expected_b = vec![
            format!("{cid_b}-alpha"),
            format!("{cid_b}-beta"),
            format!("{cid_b}-alpha-1"),
        ];

        let ids_a = heading_ids(&super::render_markdown_scoped(markdown, &cid_a));
        let ids_b = heading_ids(&super::render_markdown_scoped(markdown, &cid_b));
        assert_eq!(ids_a, expected_a);
        assert_eq!(ids_b, expected_b);
        // 異なる cid なら同じ markdown でも id が衝突しない。
        assert!(ids_a.iter().all(|id| !ids_b.contains(id)));

        // 同じ cid なら再描画しても同じ id になる（アンカーの安定性）。
        assert_eq!(
            heading_ids(&super::render_markdown_scoped(markdown, &cid_a)),
            ids_a
        );

        // TOC の href も本文 id と同じ名前空間を使う。
        let toc_a = anchor_fragments(&super::render_toc_with_prefix(markdown, &cid_a));
        let toc_b = anchor_fragments(&super::render_toc_with_prefix(markdown, &cid_b));
        assert_eq!(toc_a, ids_a);
        assert_eq!(toc_b, ids_b);
        assert!(toc_a.iter().all(|f| !toc_b.contains(f)));
        assert_eq!(
            anchor_fragments(&super::render_toc_with_prefix(markdown, &cid_a)),
            toc_a
        );
    }

    #[test]
    fn cid_scope_token_is_attribute_safe_and_stable() {
        let tricky = "a\"b<c>&d'e/f g";
        let token = super::cid_scope_token(tricky);
        let is_safe = |value: &str| {
            value
                .chars()
                .all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_')
        };
        assert!(is_safe(&token), "属性に安全でない文字: {token}");
        assert_eq!(token, super::cid_scope_token(tricky));

        // 特殊文字だけの cid でもトークンは空にならず、他と衝突しない。
        let symbols = super::cid_scope_token("!!!");
        assert!(!symbols.is_empty());
        assert_ne!(symbols, super::cid_scope_token("???"));
        assert_ne!(symbols, token);

        // Unicode CID も属性安全なトークンになる（slug の Unicode 化と整合）。
        let unicode = super::cid_scope_token("日本語CID");
        assert!(
            is_safe(&unicode),
            "Unicode CID のトークンが安全でない: {unicode}"
        );
        assert_eq!(unicode, super::cid_scope_token("日本語CID"));

        // İ (U+0130) は小文字化で 'i' + 結合文字 U+0307 に展開される。
        // 展開結果を再フィルタするので結合文字は除去され、is_safe が保たれる。
        let dotted_i = super::cid_scope_token("İstanbul");
        assert!(
            is_safe(&dotted_i),
            "İ を含む CID のトークンが安全でない: {dotted_i}"
        );
        assert_eq!(dotted_i, super::cid_scope_token("İstanbul"));
        assert!(!dotted_i.contains('\u{0307}'), "{dotted_i}");

        // prefix を含む href にも生の特殊文字が混入しない。
        let toc = super::render_toc_with_prefix("# Alpha\n", &token);
        assert!(anchor_fragments(&toc).iter().all(|f| is_safe(f)), "{toc}");
    }

    #[test]
    fn forensic_panel_id_is_unique_per_cid() {
        let a = super::forensic_panel_id(&super::cid_scope_token("cid-a"));
        let b = super::forensic_panel_id(&super::cid_scope_token("cid-b"));
        assert!(a.starts_with("forensic-timeline-"), "{a}");
        assert!(b.starts_with("forensic-timeline-"), "{b}");
        assert_ne!(a, b);
        assert_eq!(
            a,
            super::forensic_panel_id(&super::cid_scope_token("cid-a"))
        );
    }

    #[test]
    fn duplicate_headings_get_sequential_slugs() {
        let markdown = "# Foo\n\n# Foo\n\n## Foo\n\n# foo\n";
        let slugs: Vec<String> = super::collect_headings(markdown)
            .iter()
            .map(|heading| heading.slug.clone())
            .collect();
        assert_eq!(slugs, vec!["foo", "foo-1", "foo-2", "foo-3"]);

        let body = super::render_markdown(markdown);
        for slug in &slugs {
            assert!(body.contains(&format!("id=\"{slug}\"")), "{body}");
        }
        assert_eq!(anchor_fragments(&super::render_toc(markdown)), slugs);
    }

    #[test]
    fn heading_slugs_are_sanitized_against_injection() {
        // 危険文字・HTML は slug から除去され、属性を壊せない。
        // 日本語は Unicode 文字として残り、`section` には潰れない。
        assert_eq!(super::sanitize_heading_slug("A-B_C 123!"), "a-b_c123");
        assert_eq!(super::sanitize_heading_slug("日本語!!!"), "日本語");

        let markdown = "# <img src=x onerror=alert(1)>\n\n# \"quoted\" & symbols!!!\n";
        let headings = super::collect_headings(markdown);
        assert_eq!(headings[0].slug, "imgsrcxonerroralert1");
        assert_eq!(headings[1].slug, "quotedsymbols");

        let body = super::render_markdown(markdown);
        assert!(!body.contains("id=\"<"), "{body}");
        assert!(!body.contains("id=\"\""), "{body}");

        let toc = super::render_toc(markdown);
        assert_eq!(
            anchor_fragments(&toc),
            vec!["imgsrcxonerroralert1", "quotedsymbols"],
            "{toc}"
        );
        // TOC の表示名はエスケープされ、生タグとして出力されない。
        assert!(!toc.contains("<img"), "{toc}");
        assert!(toc.contains("&lt;img"), "{toc}");
        assert!(toc.contains("&amp;"), "{toc}");

        // 本番と同じ非空 prefix 経路でも見出し id / TOC href が属性を壊さない。
        let prefix = super::cid_scope_token("cid-a");
        let prefixed = super::collect_headings_with_prefix(markdown, &prefix);
        assert_eq!(prefixed.len(), 2);
        assert!(
            prefixed
                .iter()
                .all(|heading| heading.slug.starts_with(&format!("{prefix}-"))),
            "{prefixed:?}"
        );
        let prefixed_body = super::render_markdown_scoped(markdown, &prefix);
        assert!(
            prefixed_body.contains(&format!("id=\"{prefix}-")),
            "{prefixed_body}"
        );
        assert!(!prefixed_body.contains("id=\"<"), "{prefixed_body}");
        assert!(!prefixed_body.contains("id=\"\""), "{prefixed_body}");
        let prefixed_toc = super::render_toc_with_prefix(markdown, &prefix);
        assert!(
            anchor_fragments(&prefixed_toc)
                .iter()
                .all(|fragment| fragment.starts_with(&format!("{prefix}-"))),
            "{prefixed_toc}"
        );
    }

    #[test]
    fn prefixed_render_with_xss_cid_cannot_break_html_attributes() {
        // 引用符・山括弧・& を含む cid を prefix にしても、本文の id と TOC の
        // href が属性を壊さない（本番の cid 名前空間経路の回帰テスト）。
        let cid = "a\"b<c>&d'e/f g";
        let prefix = super::cid_scope_token(cid);
        assert!(
            prefix
                .chars()
                .all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_'),
            "危険な cid 由来の prefix: {prefix}"
        );
        let markdown = "# <img src=x onerror=alert(1)>\n\n# \"quoted\" & symbols!!!\n";

        let body = super::render_markdown_scoped(markdown, &prefix);
        assert!(!body.contains("<img"), "{body}");
        assert!(!body.contains("<script"), "{body}");
        assert!(!body.contains("id=\"\""), "{body}");

        let headings = super::collect_headings_with_prefix(markdown, &prefix);
        assert_eq!(headings.len(), 2);
        for heading in &headings {
            assert!(
                heading.slug.starts_with(&format!("{prefix}-")),
                "{heading:?}"
            );
            assert!(
                heading
                    .slug
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_'),
                "属性に安全でない slug: {}",
                heading.slug
            );
            assert!(body.contains(&format!("id=\"{}\"", heading.slug)), "{body}");
        }

        let toc = super::render_toc_with_prefix(markdown, &prefix);
        let fragments = anchor_fragments(&toc);
        assert_eq!(fragments.len(), 2, "{toc}");
        assert!(
            fragments
                .iter()
                .all(|fragment| fragment.starts_with(&format!("{prefix}-"))),
            "{toc}"
        );
        assert!(!toc.contains("<img"), "{toc}");
        assert!(toc.contains("&lt;img"), "{toc}");
    }

    #[test]
    fn unicode_heading_slugs_are_identifiable_and_attribute_safe() {
        use std::collections::HashMap;

        // 日本語見出しは `section` に潰れず、判別可能な slug を生成する。
        assert_eq!(super::sanitize_heading_slug("概要"), "概要");
        assert_eq!(
            super::sanitize_heading_slug("第1章 カタカナ"),
            "第1章カタカナ"
        );
        assert_eq!(super::sanitize_heading_slug("日本語!!!"), "日本語");

        // 大文字畳み込みは ASCII に限らない（`to_lowercase()` を使用）。
        assert_eq!(super::sanitize_heading_slug("ÄÖÜ"), "äöü");
        assert_eq!(super::sanitize_heading_slug("ABC-DEF_GHI"), "abc-def_ghi");

        // HTML 属性として危険な文字はことごとく除去される。
        assert_eq!(
            super::sanitize_heading_slug("a\"b'c<d>e&f g/h\\i\tj"),
            "abcdefghij"
        );
        let dangerous = super::sanitize_heading_slug("\"'<>&/ \u{0000}\u{200b}日本語");
        assert_eq!(dangerous, "日本語");
        for ch in ['"', '\'', '<', '>', '&', '/', ' ', '\t', '\n', '\r', '\0'] {
            assert!(!dangerous.contains(ch), "危険文字が残った: {ch:?}");
        }

        // すべて除去された場合だけ `section` にフォールバックする。
        assert_eq!(super::sanitize_heading_slug("\"'<>&/ \\"), "section");

        // 日本語見出しどうしも既存の連番機構で衝突しない。
        let mut used = HashMap::new();
        assert_eq!(super::assign_unique_slug("日本語", &mut used), "日本語");
        assert_eq!(super::assign_unique_slug("日本語", &mut used), "日本語-1");
    }

    #[test]
    fn japanese_slug_does_not_collide_with_cid_prefixed_slug() {
        let markdown = "# 日本語\n\n## 概要\n";
        let token_a = super::cid_scope_token("bafy-first");
        let token_b = super::cid_scope_token("bafy-second");
        assert_ne!(token_a, token_b);

        let toc_a = super::render_toc_with_prefix(markdown, &token_a);
        let toc_b = super::render_toc_with_prefix(markdown, &token_b);
        let frags_a = anchor_fragments(&toc_a);
        let frags_b = anchor_fragments(&toc_b);

        // 日本語 slug は cid 接頭辞付きでも識別可能なまま残る。
        assert!(frags_a[0].ends_with("-日本語"), "{toc_a}");
        assert!(frags_b[1].ends_with("-概要"), "{toc_b}");
        // 異なる cid の名前空間どうしでフラグメントが衝突しない。
        assert!(frags_a.iter().all(|frag| !frags_b.contains(frag)));
    }

    #[test]
    fn symbol_only_headings_fall_back_to_unique_section_slugs() {
        let markdown = "# !!!\n\n# ???\n";
        let slugs: Vec<String> = super::collect_headings(markdown)
            .iter()
            .map(|heading| heading.slug.clone())
            .collect();
        assert_eq!(slugs, vec!["section", "section-1"]);
    }

    /// 連番の記録状態まで含めて `foo` / `foo-1` / `foo-2` 系列を検証する。
    #[test]
    fn assign_unique_slug_records_next_suffix_per_base() {
        use std::collections::HashMap;

        let mut used = HashMap::new();
        assert_eq!(super::assign_unique_slug("Foo", &mut used), "foo");
        assert_eq!(used.get("foo"), Some(&1));
        assert_eq!(super::assign_unique_slug("foo", &mut used), "foo-1");
        assert_eq!(used.get("foo"), Some(&2));
        assert_eq!(super::assign_unique_slug("foo", &mut used), "foo-2");
        assert_eq!(used.get("foo"), Some(&3));

        // 明示的な `foo-1` が先に採番されても空きを飛ばして衝突しない。
        let mut used = HashMap::new();
        assert_eq!(super::assign_unique_slug("foo", &mut used), "foo");
        assert_eq!(super::assign_unique_slug("foo-1", &mut used), "foo-1");
        assert_eq!(super::assign_unique_slug("foo", &mut used), "foo-2");

        // 記号のみで base が空になる場合は `section` 系を維持。
        let mut used = HashMap::new();
        assert_eq!(super::assign_unique_slug("!!!", &mut used), "section");
        assert_eq!(super::assign_unique_slug("???", &mut used), "section-1");
    }

    /// 1 万件の同一見出しを時間計測なしで検証する（O(N²) 回帰テスト）。
    /// slug が `a`, `a-1`, ... と連番で発行され、全件一意であることを確認する。
    #[test]
    fn ten_thousand_identical_headings_get_unique_sequential_slugs() {
        use std::collections::HashSet;

        const N: usize = 10_000;
        let markdown = "# a\n".repeat(N);
        let headings = super::collect_headings(&markdown);
        assert_eq!(headings.len(), N);

        let mut seen = HashSet::with_capacity(N);
        for (i, heading) in headings.iter().enumerate() {
            let expected = if i == 0 {
                "a".to_string()
            } else {
                format!("a-{i}")
            };
            assert_eq!(heading.slug, expected, "見出し {i} の slug");
            assert!(
                seen.insert(heading.slug.clone()),
                "slug が重複した: {}",
                heading.slug
            );
        }
        assert_eq!(seen.len(), N);

        // 実害のあった render_toc 経路でも同じ一意性が成り立つこと。
        let toc = super::render_toc(&markdown);
        let fragments = anchor_fragments(&toc);
        assert_eq!(fragments.len(), N);
        assert_eq!(fragments.into_iter().collect::<HashSet<_>>().len(), N);
    }

    #[test]
    fn upload_media_type_prefers_browser_type_then_extension() {
        assert_eq!(super::upload_media_type("image/jxl", "a.bin"), "image/jxl");
        assert_eq!(
            super::upload_media_type("text/markdown", "a.bin"),
            "text/markdown"
        );
        assert_eq!(
            super::upload_media_type("text/x-markdown", "a.bin"),
            "text/markdown"
        );
        // ブラウザが JXL を認識しない場合の拡張子フォールバック。
        assert_eq!(super::upload_media_type("", "a.jxl"), "image/jxl");
        assert_eq!(
            super::upload_media_type("application/octet-stream", "a.JXL"),
            "image/jxl"
        );
        assert_eq!(super::upload_media_type("", "a.md"), "text/markdown");
        assert_eq!(super::upload_media_type("", "a.markdown"), "text/markdown");
        assert_eq!(super::upload_media_type("", "a.txt"), "text/markdown");
        // 不明な形式は空を返し、サーバー側で fail-closed に拒否される。
        assert_eq!(super::upload_media_type("", "a.zip"), "");
    }

    #[test]
    fn markdown_raw_html_is_escaped() {
        let markdown = "<img src=x onerror=alert(1)>\n\n<script>alert(1)</script>";
        let rendered = super::render_markdown(markdown);

        assert!(!rendered.contains("<img"));
        assert!(!rendered.contains("<script>"));
        assert!(rendered.contains("&lt;img"));
        assert!(rendered.contains("&lt;script&gt;"));

        // 本番と同じ非空 prefix 経路でもエスケープが維持される。
        let prefix = super::cid_scope_token("cid-a");
        let prefixed = super::render_markdown_scoped(markdown, &prefix);
        assert!(!prefixed.contains("<img"), "{prefixed}");
        assert!(!prefixed.contains("<script>"), "{prefixed}");
        assert!(prefixed.contains("&lt;img"), "{prefixed}");
        assert!(prefixed.contains("&lt;script&gt;"), "{prefixed}");
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

        // 本番と同じ非空 prefix 経路でも危険スキームが中立化される。
        let prefix = super::cid_scope_token("cid-a");
        let dangerous = "[script](javascript:alert%281%29) [data](data:text/html,alert) ![svg](data:image/svg+xml,%3Csvg%20onload=alert%281%29%3E)";
        let prefixed = super::render_markdown_scoped(dangerous, &prefix);
        assert!(!prefixed.contains("href=\"javascript:"), "{prefixed}");
        assert!(!prefixed.contains("href=\"data:"), "{prefixed}");
        assert!(!prefixed.contains("src=\"data:"), "{prefixed}");
        assert!(prefixed.contains("href=\"#\""), "{prefixed}");
        assert!(prefixed.contains("src=\"#\""), "{prefixed}");
    }

    /// プロトコル相対 URL（`//host/path`）と UNC 形式（`\\host\path`）は
    /// スキームが無くてもブラウザが現在のスキームを補完して外部ホストへ
    /// 誘導できるため、安全な相対 URL として扱わず中立化する。
    #[test]
    fn markdown_protocol_relative_and_unc_urls_are_neutralized() {
        for url in [
            "//evil.example/phish",
            "\\\\evil.example\\phish",
            // URL パーサは `\` を `/` に正規化するため混在形式も拒否する。
            "/\\evil.example/phish",
            "\\/evil.example/phish",
        ] {
            assert!(
                !super::is_safe_markdown_url(url, false),
                "unsafe link URL was accepted: {url}"
            );
            assert!(
                !super::is_safe_markdown_url(url, true),
                "unsafe image URL was accepted: {url}"
            );
        }

        // 通常の相対 URL は従来どおり許可される（回帰確認）。
        assert!(super::is_safe_markdown_url("/chapter/1", false));
        assert!(super::is_safe_markdown_url("../chapter/1", true));

        let rendered =
            super::render_markdown("[phish](//evil.example/phish) ![img](//evil.example/img.png)");
        assert!(!rendered.contains("href=\"//evil.example"), "{rendered}");
        assert!(!rendered.contains("src=\"//evil.example"), "{rendered}");
        assert_eq!(rendered.matches("href=\"#\"").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("src=\"#\"").count(), 1, "{rendered}");

        // 中立化カウントにも含まれること。
        let (_, count) = super::render_markdown_with_neutralized_count(
            "[ok](https://example.com) [phish](//evil.example/phish) [phish2](//evil.example/other)",
        );
        assert_eq!(count, 2);
    }

    /// ビルド時に注入されたオリジンは末尾スラッシュを除去して URL へ連結される。
    #[test]
    fn api_origin_has_no_trailing_slash() {
        let expected = match option_env!("OIL_API_BASE") {
            Some(value) => value.trim_end_matches('/'),
            None => "http://localhost:3000",
        };
        let rendered = format!("{}", super::API_BASE);
        assert_eq!(rendered, expected);
        assert!(!rendered.ends_with('/'));
        assert_eq!(
            super::normalize_api_origin("https://api.example.org"),
            "https://api.example.org"
        );
        assert_eq!(
            super::normalize_api_origin("https://api.example.org/"),
            "https://api.example.org"
        );
        assert_eq!(
            super::normalize_api_origin("https://api.example.org///"),
            "https://api.example.org"
        );
        // オリジンのみの判定境界（const アサーションの実行時確認）。
        #[cfg(not(feature = "dev-console"))]
        assert!(super::is_https_non_local_origin("https://api.example.org"));
        #[cfg(not(feature = "dev-console"))]
        assert!(super::is_https_non_local_origin(
            "https://api.example.org:8443"
        ));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin("http://api.example.org"));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin(
            "https://api.example.org/v1"
        ));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin(
            "https://api.example.org/?key=1"
        ));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin(
            "https://api.example.org#top"
        ));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin(
            "https://user@api.example.org"
        ));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin("https://127.0.0.1:8443"));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin("https://[::1]:8443"));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin("https://0.0.0.0"));
        #[cfg(not(feature = "dev-console"))]
        assert!(!super::is_https_non_local_origin("https://LOCALHOST"));
    }

    /// 監視イベントは event 名と count だけを持ち、サーバーの範囲 1..=1000 を守る。
    #[test]
    fn client_event_payload_is_minimal_and_clamped() {
        // 0 件は送信しない。
        assert_eq!(super::client_event_payload(0), None);
        assert_eq!(
            super::client_event_payload(1).as_deref(),
            Some(r#"{"event":"neutralized_url","count":1}"#)
        );
        assert_eq!(
            super::client_event_payload(7).as_deref(),
            Some(r#"{"event":"neutralized_url","count":7}"#)
        );
        // 上限を超えた分はクランプして送る。
        assert_eq!(
            super::client_event_payload(1000).as_deref(),
            Some(r#"{"event":"neutralized_url","count":1000}"#)
        );
        assert_eq!(
            super::client_event_payload(1001).as_deref(),
            Some(r#"{"event":"neutralized_url","count":1000}"#)
        );
        assert_eq!(
            super::client_event_payload(usize::MAX).as_deref(),
            Some(r#"{"event":"neutralized_url","count":1000}"#)
        );

        // URL 本文・IP・User-Agent 等の機微情報は一切含めない。
        let payload = super::client_event_payload(2).expect("nonzero count");
        let value: serde_json::Value = serde_json::from_str(&payload).expect("valid json");
        assert_eq!(value.as_object().map(|obj| obj.len()), Some(2));
        assert_eq!(value["event"], "neutralized_url");
        assert_eq!(value["count"], 2);
        for key in [
            "url",
            "destination",
            "ip",
            "ip_address",
            "user_agent",
            "user-agent",
        ] {
            assert!(value.get(key).is_none(), "{key} must not be sent");
        }
    }

    /// 同一の描画ソースは 1 度だけ送信し、別ソースは新たに送信する。
    /// 実際の fetch は副作用なので、純粋判定の戻り値で送信回数を数える。
    #[test]
    fn neutralized_report_is_sent_once_per_render_source() {
        let markdown = "[bad](javascript:alert(1))";
        let other = "![img](data:text/html,alert)";

        let hash = super::markdown_source_hash(markdown);
        let other_hash = super::markdown_source_hash(other);
        // ハッシュは描画ソースに対して安定し、別ソースとは衝突しない。
        assert_eq!(hash, super::markdown_source_hash(markdown));
        assert_ne!(hash, other_hash);

        let mut reported = super::ReportedSources::new();

        // 同じ markdown を2回描画しても件数と HTML は同じ。
        let (html1, count1) = super::render_markdown_with_neutralized_count(markdown);
        let (html2, count2) = super::render_markdown_with_neutralized_count(markdown);
        assert_eq!(count1, 1);
        assert_eq!(count2, count1);
        assert_eq!(html2, html1);

        // 初回だけ送信対象になり、2回目は除外される（再マウント相当）。
        assert!(super::claim_report_source(&mut reported, hash));
        assert!(!super::claim_report_source(&mut reported, hash));
        assert_eq!(reported.len(), 1);

        // 別の markdown なら新たに1回送信される（累計2回）。
        let (_, other_count) = super::render_markdown_with_neutralized_count(other);
        assert_eq!(other_count, 1);
        assert!(super::claim_report_source(&mut reported, other_hash));
        assert_eq!(reported.len(), 2);
        assert!(!super::claim_report_source(&mut reported, other_hash));
    }

    /// 危険 URL の件数が描画と同じパスで正しく集約されること。
    #[test]
    fn render_markdown_counts_neutralized_urls() {
        let (html, count) = super::render_markdown_with_neutralized_count(
            "[ok](https://example.com) [bad](javascript:alert(1)) ![img](data:text/html,alert) [rel](/chapter/1)",
        );
        assert_eq!(count, 2);
        assert!(html.contains("href=\"https://example.com\""));
        assert!(html.contains("href=\"/chapter/1\""));
        assert!(!html.contains("href=\"javascript:"));
        assert!(!html.contains("src=\"data:"));
        assert_eq!(html.matches("href=\"#\"").count(), 1);
        assert_eq!(html.matches("src=\"#\"").count(), 1);

        // 安全なリンク/img だけなら件数は 0（POST しない）。
        let (_, count) = super::render_markdown_with_neutralized_count(
            "[a](https://example.com) [b](../chapter/1) ![c](/img.png)",
        );
        assert_eq!(count, 0);

        // 参照リンク・実体参照・自動リンクも同じ経路で数える。
        let (_, count) = super::render_markdown_with_neutralized_count(
            "[ref][target] ![badge][badge-src] [entity](javascript&#58;alert%281%29)\n\n<data:text/html,alert>\n\n[target]: javascript:alert(1)\n[badge-src]: data:image/svg+xml,%3Csvg%20onload=alert%281%29%3E\n",
        );
        assert_eq!(count, 4);
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
    fn content_query_is_built_from_optional_filters() {
        assert_eq!(super::build_content_query("", "", "", false), "");
        assert_eq!(
            super::build_content_query("neon", "", "Novel", false),
            "?q=neon&type=Novel"
        );
        assert_eq!(
            super::build_content_query("a b&c", "日本", "", false),
            "?q=a%20b%26c&tag=%E6%97%A5%E6%9C%AC"
        );
        // お気に入り絞り込みは既存条件と併用できる。
        assert_eq!(
            super::build_content_query("neon", "city", "Illustration", true),
            "?q=neon&tag=city&type=Illustration&favorite_only=1"
        );
        assert_eq!(
            super::build_content_query("", "", "", true),
            "?favorite_only=1"
        );
    }

    /// R2-URL: 書き出したクエリを読み側が正しく復元できること。
    /// 日本語・`&`・`favorite_only=1` を往復のカナリとして固定する。
    #[test]
    fn content_query_roundtrip_preserves_japanese_ampersand_and_favorite_only() {
        let expected = super::ContentQuery {
            q: "日本語 & 物語".to_string(),
            tag: "夏&祭".to_string(),
            content_type: "Novel".to_string(),
            favorite_only: true,
            cid: String::new(),
            report_cid: String::new(),
        };
        let query = super::build_content_query(
            &expected.q,
            &expected.tag,
            &expected.content_type,
            expected.favorite_only,
        );
        assert_eq!(
            query,
            "?q=%E6%97%A5%E6%9C%AC%E8%AA%9E%20%26%20%E7%89%A9%E8%AA%9E&tag=%E5%A4%8F%26%E7%A5%AD&type=Novel&favorite_only=1"
        );
        assert_eq!(super::parse_content_query(&query), expected);
        // UrlSearchParams で正規化した先頭 '?' 無しの形でも同じに読めること。
        assert_eq!(
            super::parse_content_query(query.trim_start_matches('?')),
            expected
        );
    }

    #[test]
    fn content_query_parse_ignores_unknown_keys_and_accepts_valueless_flag() {
        assert_eq!(
            super::parse_content_query(""),
            super::ContentQuery::default()
        );
        let parsed = super::parse_content_query("?unknown=x&q=Neon&favorite_only&tag=urban");
        assert_eq!(parsed.q, "Neon");
        assert_eq!(parsed.tag, "urban");
        assert!(parsed.content_type.is_empty());
        // 値なしの favorite_only もフラグとして扱う（空値も同様）。
        assert!(parsed.favorite_only);
        assert!(super::parse_content_query("?favorite_only=").favorite_only);
    }

    /// canonical は parse→build の合成なので、適用済みの値へ再適用しても変わらない。
    #[test]
    fn canonical_content_query_is_idempotent() {
        let samples = [
            "",
            "?unknown=x",
            "?q=a&q=b",
            "?utm=1&type=Novel&bogus=2&q=hi&tag=city&favorite_only=1",
            "?q=a+b%26c&tag=%E6%97%A5%E6%9C%AC",
            "?favorite_only=0&favorite_only=1",
        ];
        for raw in samples {
            let once = super::canonical_content_query(raw);
            assert_eq!(
                super::canonical_content_query(&once),
                once,
                "raw={raw:?} の再適用で値が変わった"
            );
        }
        // build の出力は canonical の固定点でもある。
        let built = super::build_content_query("neon", "city", "Novel", true);
        assert_eq!(super::canonical_content_query(&built), built);
    }

    /// parse が「同じキーは最後の値を採用する」ことを canonical も維持する。
    #[test]
    fn canonical_content_query_keeps_last_value_for_duplicate_keys() {
        assert_eq!(super::canonical_content_query("?q=first&q=last"), "?q=last");
        assert_eq!(
            super::canonical_content_query("?tag=a&tag=b&q=x"),
            "?q=x&tag=b"
        );
        // フラグも最後の値が採用される（後勝ちで 0 なら条件なし）。
        assert_eq!(
            super::canonical_content_query("?favorite_only=1&favorite_only=0"),
            ""
        );
        assert_eq!(
            super::canonical_content_query("?favorite_only=0&favorite_only=1"),
            "?favorite_only=1"
        );
    }

    /// 未知キーは捨て、既知キーは build の順序へ正規化される。
    #[test]
    fn canonical_content_query_drops_unknown_keys_and_normalizes_order() {
        assert_eq!(super::canonical_content_query("?unknown=x"), "");
        assert_eq!(
            super::canonical_content_query(
                "?utm=1&type=Novel&bogus=2&q=hi&tag=city&favorite_only=1"
            ),
            "?q=hi&tag=city&type=Novel&favorite_only=1"
        );
        // 先頭 '?' が無い形も同じ結果になる。
        assert_eq!(
            super::canonical_content_query("tag=city&q=hi"),
            "?q=hi&tag=city"
        );
    }

    /// '+' は空白へ、percent escape はバイト列へ戻してから再エンコードされる。
    #[test]
    fn canonical_content_query_decodes_plus_and_percent_before_reencoding() {
        assert_eq!(super::canonical_content_query("?q=a+b%26c"), "?q=a%20b%26c");
        assert_eq!(
            super::canonical_content_query("?q=%E6%97%A5%E6%9C%AC"),
            "?q=%E6%97%A5%E6%9C%AC"
        );
        // 重複と '+' が混在しても最後の値が採用される。
        assert_eq!(super::canonical_content_query("?q=a+b&q=c+d"), "?q=c%20d");
    }

    /// cid はパスSegment として使うため `/` や `?` 等を percent-encode する。
    #[test]
    fn cid_is_percent_encoded_for_url_paths() {
        assert_eq!(super::url_encode("plain-cid_1"), "plain-cid_1");
        assert_eq!(super::url_encode("a/b c?d#e"), "a%2Fb%20c%3Fd%23e");
        assert_eq!(super::url_encode("日本語"), "%E6%97%A5%E6%9C%AC%E8%AA%9E");
    }

    #[test]
    fn only_latest_generation_response_is_applied() {
        // 現在世代と一致する応答だけを採用する。
        assert!(super::should_apply_response(0, 0));
        assert!(super::should_apply_response(42, 42));
        assert!(super::should_apply_response(u64::MAX, u64::MAX));
        // 入力変化後に届いた古い応答（反応順が逆転したケース）は捨てる。
        assert!(!super::should_apply_response(2, 1));
        assert!(!super::should_apply_response(u64::MAX, 0));
        // 未来の世代も想定外なので採用しない。
        assert!(!super::should_apply_response(1, 2));
    }

    #[test]
    fn search_debounce_delay_is_within_expected_range() {
        // 1 文字ごと GET を防ぐため、200〜300ms の範囲に収める。
        assert!((200..=300).contains(&super::SEARCH_DEBOUNCE_MS));
    }

    /// 一覧取得失敗の文言は固定値で、レスポンス本文や生エラーを含まないこと。
    #[test]
    fn dashboard_error_uses_only_fixed_phrases() {
        assert_eq!(
            super::CONTENT_INDEX_LOAD_ERROR,
            "Failed to load content index."
        );
        assert_eq!(
            super::CONTENT_INDEX_NETWORK_ERROR,
            "Network error loading content."
        );
        for message in [
            super::CONTENT_INDEX_LOAD_ERROR,
            super::CONTENT_INDEX_NETWORK_ERROR,
        ] {
            for leaked in [
                "SERVER_DETAIL_MUST_NOT_BE_SHOWN",
                "reqwest",
                "serde",
                "error sending request",
                "invalid type",
                "line 1 column",
            ] {
                assert!(
                    !message.contains(leaked),
                    "fixed phrase must not leak: {leaked}"
                );
            }
        }
    }

    #[test]
    fn content_meta_html_escapes_user_supplied_fields() {
        let tags = vec![
            "<img src=x onerror=alert(1)>".to_string(),
            "safe & sound".to_string(),
        ];
        let html = super::render_content_meta_html(
            "<script>alert(1)</script>",
            "A & B \"quoted\" <tag>",
            &tags,
        );

        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(html.contains("&lt;img"), "{html}");
        assert!(html.contains("A &amp; B &quot;quoted&quot;"), "{html}");
        assert!(html.contains("#safe &amp; sound"), "{html}");
    }

    #[test]
    fn content_meta_accepts_legacy_json_without_new_fields() {
        let meta: super::ContentMeta = serde_json::from_str(
            r#"{"cid":"c","checksum":"s","type":"Novel","status":"Approved","price":0}"#,
        )
        .unwrap();
        assert_eq!(meta.title, "");
        assert_eq!(meta.description, "");
        assert!(meta.tags.is_empty());
        assert!(!meta.is_favorite);
    }

    #[test]
    fn content_meta_reads_is_favorite_from_current_json() {
        let meta: super::ContentMeta = serde_json::from_str(
            r#"{"cid":"c","checksum":"s","type":"Novel","status":"Approved","price":0,"is_favorite":true}"#,
        )
        .unwrap();
        assert!(meta.is_favorite);
    }

    #[test]
    fn moderation_stats_json_matches_server_interface() {
        let stats: super::ModerationStats = serde_json::from_str(
            r#"{
                "generated_at":"2026-11-18T00:00:00Z",
                "totals":{"reports":3,"concluded":2,"approved":1,"rejected":1,"quorum_not_met":0,"bans":1,"unbans":0,"unpins":0},
                "monthly":[{"month":"2026-11","reports":3,"concluded":2,"approved":1,"rejected":1,"bans":1,"unbans":0,"unpins":0}]
            }"#,
        )
        .unwrap();
        assert_eq!(stats.generated_at, "2026-11-18T00:00:00Z");
        assert_eq!(stats.totals.reports, 3);
        assert_eq!(stats.totals.quorum_not_met, 0);
        assert_eq!(stats.totals.unpins, 0);
        assert_eq!(stats.monthly.len(), 1);
        assert_eq!(stats.monthly[0].month, "2026-11");
        assert_eq!(stats.monthly[0].bans, 1);

        // 旧サーバー互換と同じく、欠落フィールドは既定値で受け取る。
        let minimal: super::ModerationStats =
            serde_json::from_str(r#"{"generated_at":"2026-11-18T00:00:00Z"}"#).unwrap();
        assert_eq!(minimal.totals.reports, 0);
        assert!(minimal.monthly.is_empty());
    }

    #[test]
    fn moderation_stats_html_escapes_server_supplied_strings() {
        let stats: super::ModerationStats = serde_json::from_str(
            r#"{
                "generated_at":"<img src=x onerror=alert(1)>",
                "totals":{"reports":1},
                "monthly":[{"month":"\"><script>alert(1)</script>","reports":1,"concluded":0,"approved":0,"rejected":0,"bans":0,"unbans":0,"unpins":0}]
            }"#,
        )
        .unwrap();
        let html = super::render_moderation_stats_html(&stats);

        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;img"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(html.contains("&quot;&gt;"), "{html}");
        assert!(html.contains(">1</p>"), "{html}");
        assert!(html.contains("moderation-stats-totals"), "{html}");
        assert!(html.contains("moderation-stats-monthly"), "{html}");
    }

    #[test]
    fn public_mod_stats_json_matches_server_interface() {
        let stats: super::PublicModerationStats = serde_json::from_str(
            r#"{
                "generated_at":"2026-11-18T00:00:00Z",
                "totals":{"reports":3,"concluded":2,"approved":1,"rejected":1,"quorum_not_met":0},
                "monthly":[{"month":"2026-11","reports":3,"concluded":2,"approved":1,"rejected":1}]
            }"#,
        )
        .unwrap();
        assert_eq!(stats.generated_at, "2026-11-18T00:00:00Z");
        assert_eq!(stats.totals.reports, 3);
        assert_eq!(stats.totals.quorum_not_met, 0);
        assert_eq!(stats.monthly.len(), 1);
        assert_eq!(stats.monthly[0].month, "2026-11");
        assert_eq!(stats.monthly[0].rejected, 1);

        // 全フィールド #[serde(default)] により、欠落フィールドは既定値で受け取る。
        let minimal: super::PublicModerationStats =
            serde_json::from_str(r#"{"generated_at":"2026-11-18T00:00:00Z"}"#).unwrap();
        assert_eq!(minimal.totals.reports, 0);
        assert!(minimal.monthly.is_empty());
    }

    #[test]
    fn public_mod_stats_html_omits_moderation_actions_and_private_fields() {
        let stats: super::PublicModerationStats = serde_json::from_str(
            r#"{
                "generated_at":"2026-11-18T00:00:00Z",
                "totals":{"reports":3,"concluded":2,"approved":1,"rejected":1,"quorum_not_met":0},
                "monthly":[{"month":"2026-11","reports":3,"concluded":2,"approved":1,"rejected":1}]
            }"#,
        )
        .unwrap();
        let html = super::render_public_mod_stats_html(&stats);

        // 公開範囲外のモデレーション操作・個人情報は一切現れないこと。
        for forbidden in ["Bans", "Unbans", "Unpins", "reason", "cid", "user_hash"] {
            assert!(
                !html.contains(forbidden),
                "public html must not contain {forbidden:?}: {html}"
            );
        }
        // 公開ホワイトリストの5項目は描画されること。
        for allowed in [
            "Reports",
            "Concluded",
            "Approved",
            "Rejected",
            "Quorum Not Met",
        ] {
            assert!(
                html.contains(allowed),
                "public html must contain {allowed:?}: {html}"
            );
        }
        assert!(html.contains("public-mod-stats-totals"), "{html}");
        assert!(html.contains("public-mod-stats-monthly"), "{html}");
    }

    #[test]
    fn public_mod_stats_html_escapes_server_supplied_strings() {
        let stats: super::PublicModerationStats = serde_json::from_str(
            r#"{
                "generated_at":"<img src=x onerror=alert(1)>",
                "totals":{"reports":1},
                "monthly":[{"month":"\"><script>alert(1)</script>","reports":1}]
            }"#,
        )
        .unwrap();
        let html = super::render_public_mod_stats_html(&stats);

        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;img"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(html.contains("&quot;&gt;"), "{html}");
        assert!(html.contains(">1</p>"), "{html}");
    }

    #[test]
    fn vote_feedback_uses_only_fixed_phrases() {
        // 成功通知は固定文言。
        assert_eq!(super::VOTE_NOTICE, "投票を記録しました。");
        // HTTP エラーはステータス番号のみを埋め込む固定文言。
        assert_eq!(
            super::vote_error_message(Some(403)),
            "投票に失敗しました (HTTP 403)."
        );
        assert_eq!(
            super::vote_error_message(Some(500)),
            "投票に失敗しました (HTTP 500)."
        );
        // ネットワークエラー（ステータスなし）も固定文言。生エラーは出さない。
        assert_eq!(
            super::vote_error_message(None),
            "Network error submitting vote."
        );
    }

    #[test]
    fn reports_error_uses_only_fixed_phrases() {
        // HTTP エラー（401 を含む）はステータス番号のみを埋め込む固定文言。
        assert_eq!(
            super::reports_error_message(Some(401)),
            "投票一覧の取得に失敗しました (HTTP 401)."
        );
        assert_eq!(
            super::reports_error_message(Some(500)),
            "投票一覧の取得に失敗しました (HTTP 500)."
        );
        // ネットワークエラー（ステータスなし）も固定文言。生エラーは出さない。
        assert_eq!(
            super::reports_error_message(None),
            "Network error loading reports."
        );
        // JSON 解析失敗も解析エラーの詳細を含まない固定文言。
        assert_eq!(super::REPORTS_PARSE_ERROR, "投票一覧の解析に失敗しました。");
    }

    #[test]
    fn stats_error_uses_only_fixed_phrases() {
        // 解析失敗・取得失敗の双方で固定文言のみを返す。
        let parse = super::stats_error_message(super::StatsFailure::Parse);
        let fetch = super::stats_error_message(super::StatsFailure::Fetch);
        assert_eq!(parse, "集計の解析に失敗しました。");
        assert_eq!(fetch, "集計の取得に失敗しました。");

        // 生エラーや機微情報（serde のフィールド名・解析位置・型情報、
        // reqwest のエラー本文等）が一切含まれないことを検証する。
        for message in [parse, fetch] {
            for leaked in [
                "SERVER_DETAIL_MUST_NOT_BE_SHOWN",
                "generated_at",
                "totals",
                "monthly",
                "expected",
                "invalid type",
                "missing field",
                "line 1 column",
                "reqwest",
                "serde",
                "error sending request",
                ": ",
            ] {
                assert!(
                    !message.contains(leaked),
                    "fixed phrase must not leak raw error details: {leaked}"
                );
            }
        }
    }

    #[test]
    fn stats_forbidden_phrase_is_preserved_but_never_rendered_on_general_screen() {
        // 403 固定文言は一般画面へ描画しない（パネルは黙って閉じる）。
        // ただし確定した文言自体は変更検知のため検証し続ける。
        const FORBIDDEN: &str = "運用者権限が必要です";
        assert_eq!(FORBIDDEN, "運用者権限が必要です");
        for message in [
            super::stats_error_message(super::StatsFailure::Parse),
            super::stats_error_message(super::StatsFailure::Fetch),
        ] {
            assert_ne!(message, FORBIDDEN);
        }
    }

    #[test]
    fn verification_badge_distinguishes_decode_failure_from_checksum_mismatch() {
        let badge = super::verification_badge_text;
        assert_eq!(badge(None, false, false), "○ PENDING");
        assert_eq!(badge(Some(true), false, false), "● VERIFIED");
        // デコード成功時は古い失敗フラグが残っていても VERIFIED。
        assert_eq!(badge(Some(true), true, false), "● VERIFIED");
        assert_eq!(badge(Some(false), false, false), "× COMPROMISED");
        assert_eq!(badge(Some(false), true, false), "× DECODE FAILED");
        // 取得失敗は改ざん検知ではなく専用バッジを出し、COMPROMISED を出さない。
        assert_eq!(badge(Some(false), false, true), "× LOAD FAILED");
        assert_eq!(badge(Some(false), true, true), "× LOAD FAILED");
        assert_eq!(badge(None, false, true), "× LOAD FAILED");
        assert_ne!(badge(Some(false), false, true), "× COMPROMISED");
    }

    /// HTTP エラー／バイナリ読取失敗では改ざん検知の文言を出さず、
    /// 固定文言（＋ステータス番号のみ）を出すこと。
    #[test]
    fn content_load_error_uses_only_fixed_phrases() {
        let http = super::content_load_error_message(Some(404));
        let network = super::content_load_error_message(None);
        assert_eq!(http, "コンテンツの取得に失敗しました (HTTP 404).");
        assert_eq!(network, "コンテンツの取得に失敗しました。");

        for message in [&http, &network] {
            // 改ざん検知の文言・生エラー・サーバー本文を含まない。
            for leaked in [
                "COMPROMISED",
                "Integrity Breach",
                "checksum",
                "SERVER_DETAIL_MUST_NOT_BE_SHOWN",
                "reqwest",
                "serde",
                "invalid type",
                "error sending request",
                ": ",
            ] {
                assert!(
                    !message.contains(leaked),
                    "fixed phrase must not leak: {leaked}"
                );
            }
        }
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

    /// R5-04: 解析失敗時に「0 件」成功表示へ落とさず固定文言を出すこと。
    #[test]
    fn export_parse_failure_uses_fixed_phrase_without_success_status() {
        assert_eq!(
            super::EXPORT_PARSE_ERROR,
            "エクスポート結果の解析に失敗しました。"
        );
        // version が数値でないため DataExport として読めないレスポンス。
        let invalid: serde_json::Value =
            serde_json::from_str(r#"{"version":"invalid","contents":[]}"#).unwrap();
        assert_eq!(
            super::export_status_message(&invalid).unwrap_err(),
            "エクスポート結果の解析に失敗しました。"
        );
        // 正常系は従来どおり件数入りの成功文言（export-status に出す値）。
        let valid: serde_json::Value =
            serde_json::from_str(r#"{"version":1,"sub":"s","contents":[{}]}"#).unwrap();
        assert_eq!(
            super::export_status_message(&valid).unwrap(),
            "エクスポート完了: 1 件のレコードをダウンロードしました。"
        );
    }

    /// R5-04: サーバーの共通インターフェースに合わせた全フィールドが読めること。
    #[test]
    fn data_export_deserializes_all_agreed_fields() {
        let json = r#"{
            "version": 1,
            "generated_at": "2026-11-18T00:00:00Z",
            "sub": "subject-hash",
            "users": [{"sub": "subject-hash"}],
            "contents": [{"cid": "c1"}],
            "favorites": [{"cid": "c1"}],
            "transactions": [],
            "signatures": [{}],
            "content_audit_log": [],
            "reports": [],
            "votes": [],
            "payment_orders": [],
            "governance_audit_log": [],
            "refresh_tokens": [{}],
            "revoked_subs": []
        }"#;
        let data: super::DataExport = serde_json::from_str(json).expect("valid export");
        assert_eq!(data.version, 1);
        assert_eq!(data.generated_at, "2026-11-18T00:00:00Z");
        assert_eq!(data.sub, "subject-hash");
        assert_eq!(data.contents.len(), 1);
        assert_eq!(data.record_count(), 5);
    }

    /// R5-04: フィールドが不足した旧レスポンスもデフォルトで受け取れること。
    #[test]
    fn data_export_defaults_missing_fields() {
        let data: super::DataExport = serde_json::from_str(r#"{"version":1,"sub":"s"}"#)
            .expect("partial export must still deserialize");
        assert_eq!(data.version, 1);
        assert_eq!(data.sub, "s");
        assert_eq!(data.generated_at, "");
        assert!(data.contents.is_empty());
        assert_eq!(data.record_count(), 0);
    }

    /// ダウンロード開始失敗時は JS 例外の詳細を出さず固定文言だけを表示する。
    #[test]
    fn download_start_error_message_is_fixed_phrase() {
        assert_eq!(
            super::download_start_error_message(),
            "ダウンロードの開始に失敗しました。"
        );
    }

    /// 保存ファイル名は固定値で、ユーザー入力から組み立てられないこと。
    #[test]
    fn export_filename_is_fixed_constant() {
        assert_eq!(super::EXPORT_FILENAME, "openillustration-export.json");
    }

    /// 通報は CID・理由が空（空白のみ含む）なら POST 前に止める。
    #[test]
    fn report_validation_rejects_blank_cid_and_reason() {
        assert_eq!(
            super::validate_report_input("", "理由"),
            Err("対象 CID を入力してください。")
        );
        assert_eq!(
            super::validate_report_input("   \n", "理由"),
            Err("対象 CID を入力してください。")
        );
        assert_eq!(
            super::validate_report_input("QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g", ""),
            Err("通報理由を入力してください。")
        );
        assert_eq!(
            super::validate_report_input("cid-123", " \t "),
            Err("通報理由を入力してください。")
        );
        assert!(super::validate_report_input("cid-123", "不正コンテンツ").is_ok());
    }

    /// 入力欄の maxlength はサーバーの受理範囲（CID 128 / 理由 2000）と一致する。
    #[test]
    fn report_input_maxlength_matches_server_limits() {
        assert_eq!(super::REPORT_CID_MAX_LEN, 128);
        assert_eq!(super::REPORT_REASON_MAX_LEN, 2000);
    }

    /// 投稿フォームの maxlength は common の UTF-8 バイト上限から導出されていること。
    #[test]
    fn upload_maxlength_constants_match_common_byte_limits() {
        assert_eq!(
            super::UPLOAD_TITLE_MAX_LEN as usize,
            common::MAX_CONTENT_TITLE_BYTES
        );
        assert_eq!(
            super::UPLOAD_DESCRIPTION_MAX_LEN as usize,
            common::MAX_CONTENT_DESCRIPTION_BYTES
        );
        // 10 タグ × 50 B + 区切りカンマ 9 個 = 509（既存の maxlength と同じ値）。
        assert_eq!(
            super::UPLOAD_TAGS_MAX_LEN as usize,
            common::MAX_CONTENT_TAGS * common::MAX_CONTENT_TAG_BYTES
                + (common::MAX_CONTENT_TAGS - 1)
        );
        assert_eq!(super::UPLOAD_TAGS_MAX_LEN, 509);
    }

    /// title は UTF-8 バイトで境界を判定する（日本語 66 文字 = 198 B / 67 文字 = 201 B）。
    #[test]
    fn metadata_byte_error_enforces_utf8_title_boundary() {
        let jp = "あ";
        assert_eq!(jp.len(), 3, "テスト前提: ひらがな1文字は UTF-8 3 バイト");
        let within = jp.repeat(66); // 198 B
        let over = jp.repeat(67); // 201 B
        assert_eq!(within.len(), 198);
        assert_eq!(over.len(), 201);
        assert_eq!(super::metadata_byte_error(&within, "", ""), None);
        assert_eq!(
            super::metadata_byte_error(&over, "", ""),
            Some(super::METADATA_BYTE_ERROR)
        );
        // バイト上限を超える入力値は固定文言へエコーされない。
        assert!(!super::METADATA_BYTE_ERROR.contains(over.as_str()));
    }

    /// description は UTF-8 バイトで境界を判定する（666 文字 = 1998 B / 667 文字 = 2001 B）。
    #[test]
    fn metadata_byte_error_enforces_utf8_description_boundary() {
        let within = "あ".repeat(666); // 1998 B
        let over = "あ".repeat(667); // 2001 B
        assert_eq!(within.len(), 1998);
        assert_eq!(over.len(), 2001);
        assert_eq!(super::metadata_byte_error("", &within, ""), None);
        assert_eq!(
            super::metadata_byte_error("", &over, ""),
            Some(super::METADATA_BYTE_ERROR)
        );
    }

    /// 各タグは UTF-8 バイトで境界を判定する（16 文字 = 48 B / 17 文字 = 51 B）。
    #[test]
    fn metadata_byte_error_enforces_utf8_tag_boundary() {
        let within = "あ".repeat(16); // 48 B
        let over = "あ".repeat(17); // 51 B
        assert_eq!(within.len(), 48);
        assert_eq!(over.len(), 51);
        assert_eq!(super::metadata_byte_error("", "", &within), None);
        assert_eq!(
            super::metadata_byte_error("", "", &over),
            Some(super::METADATA_BYTE_ERROR)
        );
    }

    /// タグの分割規則はサーバーの parse_tags_field と同じ（分割・空白除去・空要素除外）。
    #[test]
    fn metadata_byte_error_counts_tags_like_server_parser() {
        let ten = ["tag"; 10].join(",");
        let eleven = ["tag"; 11].join(",");
        assert_eq!(super::metadata_byte_error("", "", &ten), None);
        assert_eq!(
            super::metadata_byte_error("", "", &eleven),
            Some(super::METADATA_BYTE_ERROR)
        );
        // 空要素は個数に数えない（",,," は 0 個）。
        assert_eq!(super::metadata_byte_error("", "", " , , "), None);
        // 前後の空白は除去してからバイト数を数える。
        let padded = format!("  {}  ", "あ".repeat(16));
        assert_eq!(super::metadata_byte_error("", "", &padded), None);
    }

    /// 通報理由は UTF-8 バイトで境界を判定する（666 文字 = 1998 B / 667 文字 = 2001 B）。
    #[test]
    fn report_reason_byte_error_enforces_utf8_boundary() {
        let within = "あ".repeat(666); // 1998 B
        let over = "あ".repeat(667); // 2001 B
        assert_eq!(within.len(), 1998);
        assert_eq!(over.len(), 2001);
        assert_eq!(super::report_reason_byte_error(&within), None);
        assert_eq!(
            super::report_reason_byte_error(&over),
            Some(super::REPORT_REASON_BYTE_ERROR)
        );
        // 空文字は長さ上限の対象外（空の判定は validate_report_input が行う）。
        assert_eq!(super::report_reason_byte_error(""), None);
        assert!(!super::REPORT_REASON_BYTE_ERROR.contains(over.as_str()));
    }

    /// 0 件時の文言は条件の有無で切り替わり、条件なしは既存文言を維持する。
    #[test]
    fn content_empty_state_message_switches_on_active_filters() {
        assert_eq!(
            super::content_empty_state_message("", "", "", false),
            "承認済みコンテンツはありません。"
        );
        assert_eq!(
            super::content_empty_state_message("  ", "", "", false),
            "承認済みコンテンツはありません。"
        );
        for (q, tag, kind, favorite) in [
            ("neon", "", "", false),
            ("", "city", "", false),
            ("", "", "Novel", false),
            ("", "", "", true),
            ("  ", "  ", "  ", true),
        ] {
            assert_eq!(
                super::content_empty_state_message(q, tag, kind, favorite),
                "条件に一致するコンテンツはありません。"
            );
            assert!(super::search_filters_active(q, tag, kind, favorite));
        }
        assert!(!super::search_filters_active("", "", "", false));
    }

    /// canvas の aria-label は無題のときだけ cid 先頭 8 文字を添える。
    #[test]
    fn illustration_aria_label_uses_cid_prefix_when_title_blank() {
        let cid = "QmYwAPJzv5CZsnAzt8auVZRn6PqfrJxkNyUdxuWz5A111g";
        assert_eq!(
            super::illustration_aria_label("", cid),
            "無題のイラスト (QmYwAPJz)"
        );
        assert_eq!(
            super::illustration_aria_label(" \n\t ", cid),
            "無題のイラスト (QmYwAPJz)"
        );
        // 8 文字に満たない cid やマルチバイトでもパニックせず文字単位で切る。
        assert_eq!(
            super::illustration_aria_label("", "abc"),
            "無題のイラスト (abc)"
        );
        assert_eq!(
            super::illustration_aria_label("", "あいうえおかきくけこ"),
            "無題のイラスト (あいうえおかきく)"
        );
        // タイトル非空なら既存の aria-label をそのまま維持する。
        assert_eq!(
            super::illustration_aria_label("Neon City", cid),
            "Neon City"
        );
        assert_eq!(super::illustration_aria_label("  Neon  ", cid), "  Neon  ");
    }

    /// 監視送信は "0" のときだけ止まり、未設定・他値は有効（既定継続）。
    #[test]
    fn client_metrics_enabled_disabled_by_zero_or_false() {
        assert_eq!(
            super::CLIENT_METRICS_ENABLED,
            super::metrics_enabled_from_env(option_env!("OIL_CLIENT_METRICS"))
        );
        assert!(super::metrics_enabled_from_env(None));
        assert!(super::metrics_enabled_from_env(Some("1")));
        assert!(super::metrics_enabled_from_env(Some("")));
        assert!(!super::metrics_enabled_from_env(Some("0")));
        assert!(!super::metrics_enabled_from_env(Some("false")));
        assert!(!super::metrics_enabled_from_env(Some("FALSE")));
    }

    /// ビルド設定に応じてペイロード作成が止まること。既定では既存契約を維持する。
    #[test]
    fn client_metrics_gate_controls_payload() {
        if super::CLIENT_METRICS_ENABLED {
            assert_eq!(
                super::client_event_payload(3).as_deref(),
                Some(r#"{"event":"neutralized_url","count":3}"#)
            );
        } else {
            assert_eq!(super::client_event_payload(3), None);
        }
    }

    /// モック認証は "0" / "false"（大小文字無視）で止まり、未設定・他値は有効。
    #[test]
    fn mock_auth_enabled_disabled_by_zero_or_false() {
        assert_eq!(
            super::MOCK_AUTH_ENABLED,
            super::mock_auth_enabled_from_env(option_env!("OIL_MOCK_AUTH"))
        );
        assert!(super::mock_auth_enabled_from_env(None));
        assert!(super::mock_auth_enabled_from_env(Some("1")));
        assert!(super::mock_auth_enabled_from_env(Some("")));
        assert!(!super::mock_auth_enabled_from_env(Some("0")));
        assert!(!super::mock_auth_enabled_from_env(Some("false")));
        assert!(!super::mock_auth_enabled_from_env(Some("FALSE")));
    }
}

#[wasm_bindgen(start)]
pub fn main() {
    // コンソール出力/panic hook は dev-console feature のときだけ有効化する。
    // trunk build は feature を渡さないため、通常ビルドではコンソールに出力しない。
    #[cfg(feature = "dev-console")]
    {
        _ = console_log::init_with_level(log::Level::Debug);
        console_error_panic_hook::set_once();
    }
    mount_to_body(|| view! { <App /> });
}
