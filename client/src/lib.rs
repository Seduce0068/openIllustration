use gloo_net::http::Request;
use leptos::prelude::*;
use leptos::task::spawn_local;
use pulldown_cmark::{CowStr, Event, HeadingLevel, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::{Blob, BlobPropertyBag, FormData, HtmlInputElement};

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

/// API オリジン。ビルド時に `OIL_API_BASE` で上書きできる表示用の型。
///
/// 値はオリジンのみ（`<scheme>://<host>[:<port>]`）とし、パス・クエリ・
/// 認証情報・機微情報は含めない。HTTPS 配信では HTTPS オリジンを指定し、
/// mixed content を避ける。未設定時は開発用ローカルの既定値を使う。
struct ApiOrigin;

/// 設定値の表記ゆれを吸収するため、末尾の `/` だけを除去する。
fn normalize_api_origin(origin: &str) -> &str {
    origin.trim_end_matches('/')
}

impl std::fmt::Display for ApiOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const DEFAULT: &str = "http://localhost:3000";
        let origin = option_env!("OIL_API_BASE").unwrap_or(DEFAULT);
        f.write_str(normalize_api_origin(origin))
    }
}

/// 各リクエスト URL の起点。`format!("{API_BASE}/...")` として使う。
const API_BASE: ApiOrigin = ApiOrigin;

/// サーバーの本文制限と同じ 10 MiB。選択時に拒否して無駄な送信を避ける。
const MAX_UPLOAD_BYTES: f64 = 10.0 * 1024.0 * 1024.0;

/// エクスポートの保存ファイル名。ユーザー入力を一切使わない固定値（パス注入対策）。
const EXPORT_FILENAME: &str = "openillustration-export.json";

/// エクスポート結果が `DataExport` として解析できない場合の固定文言。
/// 解析エラーの詳細は機微情報を含み得るため画面へ出さない。
const EXPORT_PARSE_ERROR: &str = "エクスポート結果の解析に失敗しました。";

/// `/content` 一覧の取得失敗時に表示する固定文言。
/// レスポンス本文や生エラーは機微情報を含み得るため画面へ出さない。
const CONTENT_INDEX_LOAD_ERROR: &str = "Failed to load content index.";
const CONTENT_INDEX_NETWORK_ERROR: &str = "Network error loading content.";

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
/// `web-sys` に `UrlSearchParams` を追加せず依存を増やさないため的最小実装。
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
    let _ = writeln!(
        html,
        "<p class=\"mb-4 text-[10px] text-gray-500\">GENERATED_AT: {}</p>",
        escape_html_text(&stats.generated_at)
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
            "<div class=\"border border-white/10 p-3\"><p class=\"text-[8px] uppercase tracking-widest text-gray-500\">{label}</p><p class=\"text-lg font-bold text-green-500\">{}</p></div>",
            escape_html_text(&value.to_string())
        );
    }
    html.push_str("</div>");

    html.push_str(
        "<table data-testid=\"moderation-stats-monthly\" class=\"w-full mt-6 text-left text-[10px] font-mono\">",
    );
    html.push_str(
        "<thead class=\"border-b border-white/10 text-gray-500 uppercase\"><tr>\
         <th class=\"py-2 pr-4\">Month</th><th class=\"py-2 pr-4\">Reports</th>\
         <th class=\"py-2 pr-4\">Concluded</th><th class=\"py-2 pr-4\">Approved</th>\
         <th class=\"py-2 pr-4\">Rejected</th><th class=\"py-2 pr-4\">Bans</th>\
         <th class=\"py-2 pr-4\">Unbans</th><th class=\"py-2 pr-4\">Unpins</th></tr></thead>",
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
        html.push_str("<p class=\"mt-2 text-[9px] text-gray-600 italic\">NO_MONTHLY_STATS</p>");
    }
    html
}

/// 見出しテキストから slug を作る。許可するのは ASCII 英数字・`-`・`_` のみで、
/// それ以外は除去する（属性注入を防ぐ）。大文字は小文字に畳み、
/// すべて除去された場合は `section` を返す。
fn sanitize_heading_slug(text: &str) -> String {
    let mut slug = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch.to_ascii_lowercase());
        }
    }
    if slug.is_empty() {
        "section".to_string()
    } else {
        slug
    }
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
fn collect_headings(markdown: &str) -> Vec<HeadingEntry> {
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

/// `POST /metrics/client-events` に送る JSON 本文を組み立てる（純粋関数）。
///
/// サーバーの共通インターフェースに合わせ、`event` 名と `count` 以外は一切含めない。
/// URL 本文・IP・User-Agent は送らない。件数は 1..=1000 に収め、
/// 0 件のときは送信しない（`None`）ため `spawn_local` を起こさない。
#[cfg(any(test, target_arch = "wasm32"))]
fn client_event_payload(count: usize) -> Option<String> {
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
fn render_markdown_with_neutralized_count(markdown: &str) -> (String, usize) {
    // 先に見出しを収集し、Start(Heading) の順に id を割り当てる。
    // id は pulldown-cmark の HTML レンダラがエスケープするため安全。
    let headings = collect_headings(markdown);
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

fn render_markdown(markdown: &str) -> String {
    let (html, neutralized_count) = render_markdown_with_neutralized_count(markdown);
    // 集約した件数を描画完了後に送る。同じ本文の再描画は送信側で除外される
    // （描画はブロックせず、HTML を返す契約は据え置き）。
    report_neutralized_urls(neutralized_count, markdown);
    html
}

/// 見出し一覧から TOC（アンカーリンクのリスト）を生成する。
/// リンク先は `collect_headings` が生成した `#slug` のみを使う。
fn render_toc(markdown: &str) -> String {
    let mut toc = String::new();
    for heading in collect_headings(markdown) {
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

/// ダウンロード開始に失敗したときに表示する固定文言。
/// JS 側の例外メッセージは機微情報を含み得るため表示には使わない。
fn download_start_error_message() -> &'static str {
    "ダウンロードの開始に失敗しました。"
}

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
    // 1 秒では保存開始が遅いブラウザ（低速ストレージやセキュリティスキャン等）で
    // object URL が失効し得るため、10 秒に延長する。
    let _ = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000);

    click_result?;
    Ok(())
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
                        <span data-testid="user-hash" class="text-green-500 border border-green-500/20 px-2">
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
    let (error, set_error) = signal(String::new());

    let do_login = move |_| {
        set_loading.set(true);
        set_error.set(String::new());
        let on_success = on_success.clone();
        spawn_local(async move {
            // 開発用モック認証でも nonce を取得して単回消費のフローを通す。
            // 失敗しても verify は従来どおり試み、表示するエラーは verify の結果で上書きする。
            let nonce = match Request::post(&format!("{}/auth/login", API_BASE))
                .send()
                .await
            {
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
            <Show when=move || !error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="login-error" class="text-xs text-red-500">
                    {move || error.get()}
                </p>
            </Show>
        </div>
    }
}

#[component]
fn Dashboard(token: String, user_hash: String) -> impl IntoView {
    let (items, set_items) = signal(Vec::<ContentMeta>::new());
    let (error, set_error) = signal(String::new());
    let (q, set_q) = signal(String::new());
    let (tag, set_tag) = signal(String::new());
    let (content_type, set_content_type) = signal(String::new());
    let (favorite_only, set_favorite_only) = signal(false);
    // トグル成功後に一覧を再取得するためのトリガー。
    let (refresh_counter, set_refresh_counter) = signal(0u32);
    let on_favorite_changed = move || set_refresh_counter.update(|n| *n += 1);

    let token_stored = StoredValue::new(token);
    let user_hash_stored = StoredValue::new(user_hash);

    // レスポンスの世代カウンタ。入力変化ごとに進め、古い応答は捨てる。
    // Leptos の StoredValue は SyncStorage を要求するため、ここでは
    // wasm シングルスレッド前提の Rc を Effect クロージャで直接共有する。
    let generation = Rc::new(Cell::new(0u64));
    // 直近の「即時取得トリガー」の値。テキスト入力との切り分けに使う。
    let last_immediate = Rc::new(Cell::new((false, 0u32)));
    // 実行中のデバウンスタイマー。
    let pending_fetch = Rc::new(RefCell::new(None::<PendingFetch>));
    // on_cleanup は Send+Sync を要求するため、Rc を含む PendingFetch ではなく
    // タイマーハンドル（i32）だけを StoredValue に持つ。
    let timer_handle = StoredValue::new(None::<i32>);

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
        let previous = last_immediate.get();
        // 採用した方式: q/tag/type のテキスト入力だけ 250ms デバウンスする。
        // favorite_only の変更と refresh_counter（お気に入りトグルの成功通知）は
        // 即時取得する。これらはユーザーの明示操作なので待たせず、
        // かつ連続テキスト入力による 1 文字ごとの GET を防ぐ。
        let immediate = favorite != previous.0 || refresh != previous.1;
        last_immediate.set((favorite, refresh));

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
                match Request::get(&format!("{}/content{}", API_BASE, query))
                    .header("Authorization", &format!("Bearer {}", token))
                    .send()
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
                    <h2 class="text-xs font-bold tracking-widest uppercase">"MY DATA"</h2>
                    <p class="text-[9px] text-gray-500 mt-1">
                        "DOWNLOAD ALL OF YOUR DATA AS JSON."
                    </p>
                </div>
                <ExportMyData token=token_stored.get_value() />
            </section>
            <search>
                <form
                    on:submit=move |ev| ev.prevent_default()
                    class="grid grid-cols-1 md:grid-cols-[1fr_1fr_auto_auto_auto] gap-4 items-end border border-white/10 p-4"
                >
                    <div class="space-y-1">
                        <label for="content-q" class="block text-[9px] uppercase tracking-widest text-gray-500">
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
                        <label for="content-tag" class="block text-[9px] uppercase tracking-widest text-gray-500">
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
                        <label for="content-type" class="block text-[9px] uppercase tracking-widest text-gray-500">
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
                        <label for="favorite-only"
                            class="text-[9px] uppercase tracking-widest text-gray-500 cursor-pointer">
                            "Favorites only"
                        </label>
                    </div>
                    <button type="submit"
                        class="py-3 px-6 border border-white/20 text-[10px] uppercase tracking-widest hover:bg-white/10 transition-colors">
                        "Search"
                    </button>
                </form>
            </search>
            <p role="status" aria-live="polite" class="sr-only">
                {move || format!("{} content items displayed", items.get().len())}
            </p>
            // 一覧取得の失敗は items が前回成功値を保持していても常に表示する。
            // （空状態の中に出すと items が残っているときにエラーが消えていた。）
            <Show when=move || !error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="dashboard-error" class="text-xs text-red-500 mb-4">
                    {move || error.get()}
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
                                user_hash=user_hash_stored.get_value()
                                title=item.title.clone()
                                description=item.description.clone()
                                tags=item.tags.clone()
                                is_favorite=item.is_favorite
                                on_favorite_changed=on_favorite_changed
                            />
                        }
                    }
                />
                <Show when=move || items.get().is_empty() fallback=|| ()>
                    <div class="border border-white/10 p-4 opacity-50 flex items-center justify-center italic text-[10px] text-gray-600 col-span-full">
                        "NO_APPROVED_CONTENT"
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
    let token_stored = StoredValue::new(token);

    let on_export = move |_| {
        let token = token_stored.get_value();
        set_error.set(String::new());
        set_status.set(String::new());
        set_busy.set(true);
        spawn_local(async move {
            let result: Result<String, String> = async {
                let response = Request::get(&format!("{}/me/export", API_BASE))
                    .header("Authorization", &format!("Bearer {}", token))
                    .send()
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
                download_json_file(&payload, EXPORT_FILENAME)?;
                Ok(message)
            }
            .await;
            set_busy.set(false);
            match result {
                Ok(message) => set_status.set(message),
                Err(message) => set_error.set(message),
            }
        });
    };

    view! {
        <div class="space-y-2 text-right">
            <button
                type="button"
                on:click=on_export
                disabled=busy
                aria-label="EXPORT MY DATA"
                data-testid="export-my-data"
                class="py-3 px-6 border border-white/20 text-[10px] uppercase tracking-widest hover:bg-white/10 transition-colors disabled:opacity-50"
            >
                {move || if busy.get() { "EXPORTING..." } else { "EXPORT MY DATA" }}
            </button>
            <Show when=move || !error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="export-error" class="text-xs text-red-500">
                    {move || error.get()}
                </p>
            </Show>
            <Show when=move || !status.get().is_empty() fallback=|| ()>
                <p role="status" data-testid="export-status" class="text-xs text-gray-400">
                    {move || status.get()}
                </p>
            </Show>
        </div>
    }
}

mod jxl;

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
fn ContentViewer<F>(
    cid: String,
    expected_checksum: String,
    content_type: String,
    price: i64,
    token: String,
    #[allow(unused_variables)] user_hash: String,
    title: String,
    description: String,
    tags: Vec<String>,
    is_favorite: bool,
    on_favorite_changed: F,
) -> impl IntoView
where
    F: Fn() + 'static + Clone,
{
    let cid_label = cid.clone();
    // メタデータは信頼できない入力なので、escape_html_text を通した文字列だけを
    // inner_html へ渡す（Markdown 本文/TOC の既存レンダリングには触れない）。
    let meta_html = render_content_meta_html(&title, &description, &tags);
    let (verified, set_verified) = signal(None::<bool>);
    // チェックサムは正常だが JXL のデコードに失敗した場合を改ざん検知と区別する。
    let (decode_error, set_decode_error) = signal(false);
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
    // モーダル化: ダイアログ内の初期フォーカス先兼 Tab トラップの周回先。
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
            let res = if turning_on {
                Request::post(&url)
            } else {
                Request::delete(&url)
            }
            .header("Authorization", &format!("Bearer {}", token))
            .send()
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

    let ct_ef = content_type.clone();
    Effect::new(move |_| {
        let cid = cid.clone();
        let expected_checksum = expected_checksum.clone();
        let ct = ct_ef.clone();
        let token = token_stored.get_value();
        spawn_local(async move {
            // 再取得や cid 切り替えで前回の失敗表示が残らないようにする。
            set_decode_error.set(false);
            set_content_load_error.set(String::new());
            let url = format!("{}/content/{}", API_BASE, url_encode(&cid));
            let res = Request::get(&url)
                .header("Authorization", &format!("Bearer {}", token))
                .send()
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
                                let text = String::from_utf8_lossy(&bytes).to_string();
                                // 本文と TOC は同じテキストから生成し、アンカーの一致を保証する。
                                set_novel_toc.set(render_toc(&text));
                                set_novel_html.set(render_markdown(&text));
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

            match Request::get(&format!("{}/content/{}/audit", API_BASE, url_encode(&cid)))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
                .await
            {
                Ok(response) if response.ok() => match response.json::<Vec<AuditLog>>().await {
                    Ok(logs) => {
                        set_audit_error.set(String::new());
                        set_audit_logs.set(logs);
                    }
                    Err(_) => set_audit_error.set("監査ログの解析に失敗しました。".to_string()),
                },
                Ok(response) => set_audit_error.set(format!(
                    "監査ログの取得に失敗しました (HTTP {}).",
                    response.status()
                )),
                Err(_) => set_audit_error.set("Network error loading audit log.".to_string()),
            }
        });
    });

    let ct_can = content_type.clone();
    let ct_nov = content_type.clone();
    let ct_bad = content_type.clone();
    let ct_toc = content_type.clone();

    view! {
        <div data-testid="content-card"
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                // ダイアログ内（トグル含む）で Escape が押されたら閉じてフォーカスを戻す。
                if ev.key() == "Escape" && show_audit.get() {
                    set_show_audit.set(false);
                    if let Some(toggle) = audit_toggle_ref.get() {
                        let _ = toggle.focus();
                    }
                    return;
                }
                // モーダル中の Tab トラップ。パネル内のインタラクティブ要素は
                // CLOSE ボタンのみなので、Tab / Shift+Tab を CLOSE へ戻し、
                // 背後のトグルやカード要素へフォーカスが抜けないようにする。
                if ev.key() == "Tab" && show_audit.get() {
                    ev.prevent_default();
                    if let Some(close) = audit_close_ref.get() {
                        let _ = close.focus();
                    }
                }
            }
            class="p-4 border border-white/10 bg-black text-white space-y-4 flex flex-col h-full">
            <div class="flex justify-between items-start">
                <p class="text-[8px] text-gray-600 truncate flex-1 mr-2">{cid_label.clone()}</p>
                <div class="flex gap-2">
                    <button
                        type="button"
                        on:click=toggle_favorite
                        data-testid="favorite-toggle"
                        disabled=move || favorite_busy.get()
                        aria-label=move || if favorite.get() { "Remove from favorites" } else { "Add to favorites" }
                        aria-pressed=move || if favorite.get() { "true" } else { "false" }
                        class="text-[10px] border border-white/20 px-1 hover:bg-white/10 transition-colors disabled:opacity-50">
                        {move || if favorite.get() { "★" } else { "☆" }}
                    </button>
                    <button on:click=move |_| set_show_audit.update(|v| *v = !*v)
                        node_ref=audit_toggle_ref
                        aria-expanded=move || if show_audit.get() { "true" } else { "false" }
                        aria-controls="forensic-timeline"
                        class="text-[8px] border border-white/20 px-1 hover:bg-white/10 transition-colors uppercase">
                        "Audit"
                    </button>
                </div>
            </div>
            <Show when=move || !favorite_error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="favorite-error" class="text-[8px] text-red-500">{move || favorite_error.get()}</p>
            </Show>
            <div class="border-b border-white/10 pb-3" inner_html=meta_html />

            <Show when=move || verified.get() == Some(true) && ct_toc == "Novel" && !novel_toc.get().is_empty() fallback=|| ()>
                <details class="border border-white/10 bg-white/5 p-3">
                    <summary class="text-[10px] tracking-widest uppercase text-gray-400 hover:text-white cursor-pointer">
                        "Table of Contents"
                    </summary>
                    <nav class="mt-3 space-y-1 max-h-40 overflow-y-auto" inner_html=move || novel_toc.get() />
                </details>
            </Show>

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
                        <div class="text-center p-4" data-testid="content-verification-error">
                            <Show when=move || !content_load_error.get().is_empty() fallback=|| ()>
                                <p role="alert" data-testid="content-load-error" class="text-red-500 font-bold text-[10px] mb-2 uppercase">
                                    "Content Load Failed"
                                </p>
                                <p data-testid="content-load-error-detail" class="text-gray-500 text-[8px]">
                                    {move || content_load_error.get()}
                                </p>
                            </Show>
                            <Show when=move || content_load_error.get().is_empty() && decode_error.get() fallback=|| ()>
                                <p role="alert" data-testid="jxl-decode-error" class="text-red-500 font-bold text-[10px] mb-2 uppercase">
                                    "JXL Decode Failed"
                                </p>
                                <p data-testid="jxl-decode-error-detail" class="text-gray-500 text-[8px]">
                                    "JXL のデコードに失敗しました。"
                                </p>
                            </Show>
                            <Show when=move || content_load_error.get().is_empty() && !decode_error.get() fallback=|| ()>
                                <p class="text-red-500 font-bold text-[10px] mb-2 uppercase">"Integrity Breach Detected"</p>
                                <p class="text-gray-500 text-[8px]">"Wasm checksum does not match forensic registry."</p>
                            </Show>
                        </div>
                    }.into_any(),
                    None => view! { <span class="text-[10px] text-gray-600 animate-pulse uppercase tracking-tighter">"Verifying_Integrity..."</span> }.into_any(),
                }}

                <Show when=move || show_audit.get() fallback=|| ()>
                    <div id="forensic-timeline"
                        role="dialog"
                        aria-modal="true"
                        aria-label="Forensic Timeline"
                        class="absolute inset-0 bg-black/95 z-20 p-4 overflow-y-auto border border-white/20">
                        <h4 class="text-[10px] font-bold mb-4 border-b border-white/10 pb-2">"FORENSIC_TIMELINE"</h4>
                        <Show when=move || !audit_error.get().is_empty() fallback=|| ()>
                            <p role="alert" data-testid="audit-error" class="text-[8px] text-red-500">
                                {move || audit_error.get()}
                            </p>
                        </Show>
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
                        <button on:click=move |_| {
                                set_show_audit.set(false);
                                // ダイアログを閉じた後、呼び出し元のトグルへフォーカスを戻す。
                                if let Some(toggle) = audit_toggle_ref.get() {
                                    let _ = toggle.focus();
                                }
                            }
                            node_ref=audit_close_ref
                            class="mt-4 w-full py-2 border border-white/20 text-[8px] hover:bg-white/10">
                            "CLOSE"
                        </button>
                    </div>
                </Show>
            </div>
            <div class="text-[10px] tracking-widest font-bold mt-auto flex justify-between items-center">
                <div class="flex items-center gap-4">
                    <span data-testid="content-verification-badge" class=move || match verified.get() {
                        Some(true) => "text-green-500",
                        Some(false) => "text-red-500",
                        None => "text-gray-600",
                    }>
                        {move || verification_badge_text(
                            verified.get(),
                            decode_error.get(),
                            !content_load_error.get().is_empty(),
                        )}
                    </span>
                    <span class="text-gray-500 border border-gray-500/20 px-1">{ct_bad.to_uppercase()}</span>
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

#[component]
fn Upload(user_hash: String, token: String) -> impl IntoView {
    let (status, set_status) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (file, set_file) = signal(None::<web_sys::File>);
    let (price, set_price) = signal(100);
    let (title, set_title) = signal(String::new());
    let (description, set_description) = signal(String::new());
    let (tags, set_tags) = signal(String::new());
    // ラベルをキーボードで操作したときに file input へclick()を渡すための参照。
    let file_input_ref = NodeRef::<leptos::html::Input>::new();

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
        } else {
            // 選択解除（キャンセル）時は古い File を破棄し、送信ボタンを無効化する。
            set_file.set(None);
            set_status.set(String::new());
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
        let title_val = title.get();
        let description_val = description.get();
        let tags_val = tags.get();
        set_loading.set(true);
        set_status.set("Generating forensic evidence...".to_string());

        // 失敗時に表示する文言は固定文字列のみ（生データ・機微情報は出さない）。
        const PREPARE_ERROR: &str = "アップロードの準備に失敗しました。";
        const NETWORK_ERROR: &str = "ネットワークエラーが発生しました。";
        const RESPONSE_ERROR: &str = "アップロード結果の読み取りに失敗しました。";

        spawn_local(async move {
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

                let response = Request::post(&format!("{}/content/upload", API_BASE))
                    .header("Authorization", &format!("Bearer {}", token))
                    .body(form_data)
                    .map_err(|_| PREPARE_ERROR)?
                    .send()
                    .await
                    .map_err(|_| NETWORK_ERROR)?;

                if !response.ok() {
                    return Ok("ERROR: Upload failed.".to_string());
                }
                let cid = response
                    .json::<String>()
                    .await
                    .map_err(|_| RESPONSE_ERROR)?;
                Ok(format!("SUCCESS: Content registered as {}", cid))
            }
            .await;

            match outcome {
                Ok(message) => set_status.set(message),
                Err(message) => set_status.set(message.to_string()),
            }
            // finally 相当: 成否にかかわらずボタンを再活性化する。
            set_loading.set(false);
        });
    };

    view! {
        <div class="max-w-2xl mx-auto mt-12 border border-white/10 p-8">
            <h2 class="text-xl font-bold mb-6 tracking-widest uppercase">"Forensic Upload Engine (Rust)"</h2>
            <div class="space-y-6">
                <div class="border-2 border-dashed border-white/20 p-12 text-center hover:border-white/40 transition-colors">
                    // display:none の代わりに 1px + clip で隠し、フォーカス可能なままにする。
                    <input type="file" on:change=on_file_change
                        node_ref=file_input_ref
                        class="absolute h-px w-px overflow-hidden whitespace-nowrap border-0 p-0 clip-[rect(0,0,0,0)]"
                        id="file-upload" />
                    <label for="file-upload"
                        role="button"
                        tabindex="0"
                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                            if is_file_picker_activation_key(&ev.key()) {
                                ev.prevent_default();
                                if let Some(input) = file_input_ref.get() {
                                    input.click();
                                }
                            }
                        }
                        class="cursor-pointer">
                        {move || file.get().map(|f| f.name()).unwrap_or_else(|| "SELECT JXL OR MARKDOWN".to_string())}
                    </label>
                </div>

                <div class="space-y-2">
                    <label for="upload-price" class="text-[10px] text-gray-500 uppercase tracking-widest">"Support Price (JPYC)"</label>
                    <input type="number"
                        id="upload-price"
                        name="price"
                        min="1"
                        max="1000000"
                        on:input=move |ev| set_price.set(event_target_value(&ev).parse().unwrap_or(0))
                        prop:value=price
                        class="w-full bg-black border border-white/20 p-4 text-xl font-mono text-green-500" />
                </div>

                <div class="space-y-1">
                    <label for="upload-title" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "Title (optional)"
                    </label>
                    <input
                        id="upload-title"
                        name="title"
                        type="text"
                        maxlength=200
                        placeholder="TITLE"
                        on:input=move |ev| set_title.set(event_target_value(&ev))
                        prop:value=title
                        class="w-full bg-black border border-white/20 p-3 text-sm"
                    />
                </div>

                <div class="space-y-1">
                    <label for="upload-description" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "Description (optional)"
                    </label>
                    <textarea
                        id="upload-description"
                        name="description"
                        maxlength=2000
                        placeholder="DESCRIPTION"
                        on:input=move |ev| set_description.set(event_target_value(&ev))
                        prop:value=description
                        class="w-full bg-black border border-white/20 p-3 text-sm h-24 resize-y"
                    />
                </div>

                <div class="space-y-1">
                    <label for="upload-tags" class="block text-[10px] text-gray-500 uppercase tracking-widest">
                        "Tags (optional, comma separated)"
                    </label>
                    <input
                        id="upload-tags"
                        name="tags"
                        type="text"
                        maxlength=509
                        placeholder="FANTASY, SCIFI"
                        on:input=move |ev| set_tags.set(event_target_value(&ev))
                        prop:value=tags
                        class="w-full bg-black border border-white/20 p-3 text-sm"
                    />
                </div>

                <button on:click=sign_and_upload disabled=move || file.get().is_none() || loading.get()
                    class="w-full py-4 bg-white text-black font-bold uppercase tracking-widest hover:bg-gray-200 transition-colors disabled:opacity-50">
                    {move || if loading.get() { "Processing..." } else { "Sign & Submit to Evidence Ledger" }}
                </button>
                <Show when=move || !status.get().is_empty() fallback=|| ()>
                    <div data-testid="upload-status" role="status" aria-live="polite" class="p-4 bg-white/5 border border-white/20 text-xs">
                        <span class="text-gray-500 mr-2">"[STATUS]"</span> {move || status.get()}
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// `/transactions` の取得失敗時に表示する固定文言。
/// レスポンス本文や生エラー（reqwest / serde_json）は機微情報を含み得るため出さない。
const LEDGER_ERROR: &str = "取引履歴の取得に失敗しました。";

#[component]
fn Ledger(token: String) -> impl IntoView {
    let (txs, set_txs) = signal(Vec::<Transaction>::new());
    // 取得失敗を明示する。失敗時は「NO_TRANSACTIONS_RECORDED」を出さず、
    // 固定文言のみを表示する。
    let (ledger_error, set_ledger_error) = signal(String::new());

    Effect::new(move |_| {
        let token = token.clone();
        spawn_local(async move {
            // 再取得時に前回の失敗表示が残らないようにする。
            set_ledger_error.set(String::new());
            match Request::get(&format!("{}/transactions", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
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
                // HTTP エラー・ネットワークエラー: ステータスや本文、生エラーは出さない。
                _ => set_ledger_error.set(LEDGER_ERROR.to_string()),
            }
        });
    });

    view! {
        <div class="max-w-5xl mx-auto mt-12">
            <h2 class="text-xl font-bold tracking-widest uppercase mb-8">"Forensic Transaction Ledger"</h2>
            <div class="border border-amber-500/30 bg-amber-500/5 text-amber-300/90 text-[9px] p-3 mb-4 font-mono leading-relaxed">
                "LEGACY MOCK RECORDS — Written by the disabled mock support API. These entries are not proof of payment; JPYC settlement is not implemented."
            </div>
            <Show when=move || !ledger_error.get().is_empty() fallback=|| ()>
                <p role="alert" data-testid="ledger-error" class="text-xs text-red-500 mb-4">
                    {move || ledger_error.get()}
                </p>
            </Show>
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
                <Show when=move || txs.get().is_empty() && ledger_error.get().is_empty() fallback=|| ()>
                    <div class="p-12 text-center text-gray-600 italic">"NO_TRANSACTIONS_RECORDED"</div>
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

#[component]
fn Moderation(token: String, user_hash: String) -> impl IntoView {
    let (cid, set_cid) = signal(String::new());
    let (reason, set_reason) = signal(String::new());
    let (status, set_status) = signal(String::new());
    let (reports, set_reports) = signal(Vec::<Report>::new());
    // 投票一覧の取得失敗を明示する。失敗しても reports は空にせず既存データを維持し、
    // 空状態を「保留投票なし」と誤表示しない。
    let (reports_error, set_reports_error) = signal(String::new());
    // R5-09: 透明性ダッシュボードの集計表示状態。
    let (stats_html, set_stats_html) = signal(String::new());
    let (stats_error, set_stats_error) = signal(String::new());
    let (stats_loaded, set_stats_loaded) = signal(false);
    // 投票結果のフィードバック。成功通知と失敗エラーは固定文言のみを表示する。
    let (vote_error, set_vote_error) = signal(String::new());
    let (vote_notice, set_vote_notice) = signal(String::new());

    let token_stored = StoredValue::new(token);
    let user_hash_stored = StoredValue::new(user_hash);

    let fetch_reports = move || {
        let token = token_stored.get_value();
        spawn_local(async move {
            match Request::get(&format!("{}/mod/reports", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
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

    // 運用者限定の集計を取得する。403 は権限エラーとして明示し、
    // それ以外の失敗（HTTP エラー・JSON 解析・ネットワーク）も握りつぶさず表示する。
    Effect::new(move |_| {
        let token = token_stored.get_value();
        spawn_local(async move {
            match Request::get(&format!("{}/mod/stats", API_BASE))
                .header("Authorization", &format!("Bearer {}", token))
                .send()
                .await
            {
                Ok(response) if response.ok() => match response.json::<ModerationStats>().await {
                    Ok(stats) => {
                        set_stats_html.set(render_moderation_stats_html(&stats));
                        set_stats_error.set(String::new());
                        set_stats_loaded.set(true);
                    }
                    Err(_) => {
                        // 解析エラーの詳細（フィールド名・位置・生エラー）は出さず固定文言のみ表示する。
                        set_stats_html.set(String::new());
                        set_stats_loaded.set(false);
                        set_stats_error.set(stats_error_message(StatsFailure::Parse).to_string());
                    }
                },
                Ok(response) if response.status() == 403 => {
                    set_stats_html.set(String::new());
                    set_stats_loaded.set(false);
                    set_stats_error.set("運用者権限が必要です".to_string());
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
        // 新しい試行では前回の通知・エラーを必ず消す。
        set_vote_error.set(String::new());
        set_vote_notice.set(String::new());
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
        });
    };

    view! {
        <div class="max-w-4xl mx-auto mt-12 grid grid-cols-1 md:grid-cols-2 gap-12">
            <section data-testid="moderation-stats-section" class="md:col-span-2 border border-white/10 p-8 space-y-4">
                <h2 class="text-xl font-bold tracking-widest uppercase">"TRANSPARENCY DASHBOARD"</h2>
                <Show when=move || !stats_error.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="moderation-stats-error" class="text-xs text-red-500">
                        {move || stats_error.get()}
                    </p>
                </Show>
                <Show when=move || stats_loaded.get() fallback=|| ()>
                    <div data-testid="moderation-stats" inner_html=move || stats_html.get() />
                </Show>
            </section>
            <div class="border border-white/10 p-8 space-y-6 h-fit">
                <h2 class="text-xl font-bold tracking-widest uppercase">"Submit Report"</h2>
                <p class="text-[10px] text-gray-500">
                    "Reporter: " {move || user_hash_stored.get_value()[0..8.min(user_hash_stored.get_value().len())].to_string()}
                </p>
                <input
                    type="text"
                    aria-label="TARGET CID"
                    placeholder="TARGET CID"
                    on:input=move |ev| set_cid.set(event_target_value(&ev))
                    class="w-full bg-black border border-white/20 p-3 text-sm"
                />
                <textarea
                    aria-label="REASON"
                    placeholder="REASON"
                    on:input=move |ev| set_reason.set(event_target_value(&ev))
                    class="w-full bg-black border border-white/20 p-3 text-sm h-24"
                />
                <button on:click=submit_report
                    class="w-full py-3 border border-white hover:bg-white hover:text-black font-bold text-xs tracking-widest">
                    "SUBMIT FORENSIC REPORT"
                </button>
                <Show when=move || !status.get().is_empty() fallback=|| ()>
                    <p role="status" class="text-xs text-gray-400">{move || status.get()}</p>
                </Show>
            </div>

            <div class="border border-white/10 p-8 space-y-6">
                <h2 class="text-xl font-bold tracking-widest uppercase">"Jury Voting Queue"</h2>
                <Show when=move || !reports_error.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="reports-error" class="text-xs text-red-500">
                        {move || reports_error.get()}
                    </p>
                </Show>
                <Show when=move || !vote_error.get().is_empty() fallback=|| ()>
                    <p role="alert" data-testid="vote-error" class="text-xs text-red-500">
                        {move || vote_error.get()}
                    </p>
                </Show>
                <Show when=move || !vote_notice.get().is_empty() fallback=|| ()>
                    <p role="status" data-testid="vote-notice" class="text-xs text-green-500">
                        {move || vote_notice.get()}
                    </p>
                </Show>
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
                    <Show when=move || reports.get().is_empty() && reports_error.get().is_empty() fallback=|| ()>
                        <div class="text-center py-12 text-[10px] text-gray-600 italic">"NO_PENDING_VOTES"</div>
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
    fn file_picker_activates_only_on_enter_or_space() {
        assert!(super::is_file_picker_activation_key("Enter"));
        assert!(super::is_file_picker_activation_key(" "));
        assert!(super::is_file_picker_activation_key("Spacebar"));
        assert!(!super::is_file_picker_activation_key("Escape"));
        assert!(!super::is_file_picker_activation_key("a"));
    }

    #[test]
    fn favorite_error_message_uses_dedicated_401_text() {
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
        // 危険文字・HTML ・日本語は slug から除去され、属性を壊せない。
        assert_eq!(super::sanitize_heading_slug("A-B_C 123!"), "a-b_c123");
        assert_eq!(super::sanitize_heading_slug("日本語!!!"), "section");

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

        // 日本語などで base が空になる場合は `section` 系を維持。
        let mut used = HashMap::new();
        assert_eq!(super::assign_unique_slug("日本語", &mut used), "section");
        assert_eq!(super::assign_unique_slug("!!!", &mut used), "section-1");
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
}

#[wasm_bindgen(start)]
pub fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App /> });
}
