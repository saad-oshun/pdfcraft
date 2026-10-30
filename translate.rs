//! Translate selected text to Arabic.
//!
//! The text selection bar's **Translate** button sends the selected words to Google Translate's
//! free web endpoint (no key, no account) on a worker thread, and a small window shows the Arabic
//! result with Copy and Close. Nothing is sent until the button is pressed. The web build has no
//! network client, so there the window says translation is unavailable.

use std::sync::{Arc, Mutex};

/// The most characters sent in one request: a long selection is cut at a char boundary.
const MAX_CHARS: usize = 4_500;

#[derive(Clone)]
enum Status {
    Pending,
    Done(String),
    Failed(String),
}

#[derive(Clone)]
struct Job {
    source: String,
    status: Arc<Mutex<Status>>,
}

fn key() -> egui::Id {
    egui::Id::unique("pdfcraft-arabic-translation")
}

/// Start translating `text` to Arabic and open the result window (replacing any earlier one).
pub(crate) fn start(ctx: &egui::Context, text: &str) {
    let source: String = text.trim().chars().take(MAX_CHARS).collect();
    if source.is_empty() {
        return;
    }
    let status = Arc::new(Mutex::new(Status::Pending));
    let job = Job { source: source.clone(), status: status.clone() };
    ctx.data_mut(|d| d.insert_temp(key(), Some(job)));
    spawn(ctx.clone(), source, status);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(ctx: egui::Context, source: String, status: Arc<Mutex<Status>>) {
    let worker = status.clone();
    let spawned = std::thread::Builder::new().name("translate".into()).spawn(move || {
        // A panic in the request must not take the app down: it becomes an error message.
        let result = std::panic::catch_unwind(|| fetch(&source)).unwrap_or_else(|_| Err("Translation failed unexpectedly.".into()));
        let next = match result {
            Ok(text) => Status::Done(text),
            Err(e) => Status::Failed(e),
        };
        // A poisoned lock still holds a usable value: take it rather than give up.
        let mut guard = worker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = next;
        drop(guard);
        ctx.request_repaint();
    });
    if spawned.is_err()
        && let Ok(mut guard) = status_lock(&status)
    {
        *guard = Status::Failed("Couldn't start the translation.".into());
    }
}

#[cfg(target_arch = "wasm32")]
fn spawn(_ctx: egui::Context, _source: String, status: Arc<Mutex<Status>>) {
    if let Ok(mut guard) = status_lock(&status) {
        *guard = Status::Failed("Translation isn't available in the web version.".into());
    }
}

fn status_lock(status: &Arc<Mutex<Status>>) -> Result<std::sync::MutexGuard<'_, Status>, ()> {
    Ok(status.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Ask Google Translate's free endpoint for the Arabic of `text` (source language detected).
#[cfg(not(target_arch = "wasm32"))]
fn fetch(text: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(20))).build().into();
    let mut response = agent
        .get("https://translate.googleapis.com/translate_a/single")
        .query("client", "gtx")
        .query("sl", "auto")
        .query("tl", "ar")
        .query("dt", "t")
        .query("q", text)
        .call()
        .map_err(|e| format!("Couldn't reach the translation service ({e}). Check your internet connection."))?;
    let body =
        response.body_mut().with_config().limit(4 * 1024 * 1024).read_to_string().map_err(|e| format!("Couldn't read the translation ({e})."))?;
    parse(&body).ok_or_else(|| "The translation service sent an answer PdfCraft couldn't read.".into())
}

/// The translated text in the endpoint's answer: `[[["ترجمة","source",…],…],…]`, the first
/// string of every sentence joined in order.
fn parse(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let sentences = value.get(0)?.as_array()?;
    let out: String = sentences.iter().filter_map(|s| s.get(0).and_then(serde_json::Value::as_str)).collect();
    (!out.trim().is_empty()).then_some(out)
}

/// Show the translation window, if one is open. Call once per frame.
pub(crate) fn show(ctx: &egui::Context) {
    let Some(Some(job)) = ctx.data(|d| d.get_temp::<Option<Job>>(key())) else { return };
    let status = job.status.lock().map(|s| s.clone()).unwrap_or_else(|p| p.into_inner().clone());
    let mut open = true;
    let mut close = false;
    egui::Window::new("الترجمة إلى العربية")
        .id(key().with("window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .max_height(ctx.content_rect().height() * 0.7)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                ui.label(egui::RichText::new(&job.source).weak());
                ui.separator();
                match &status {
                    Status::Pending => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("جارٍ الترجمة…");
                        });
                    }
                    Status::Done(text) => {
                        ui.add(egui::Label::new(egui::RichText::new(text).size(16.0)).wrap().selectable(true));
                    }
                    Status::Failed(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if let Status::Done(text) = &status
                    && ui.button("نسخ").clicked()
                {
                    ui.ctx().copy_text(text.clone());
                }
                if ui.button("إغلاق").clicked() {
                    close = true;
                }
            });
        });
    if !open || close {
        ctx.data_mut(|d| d.insert_temp::<Option<Job>>(key(), None));
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn joins_every_sentence_of_the_answer() {
        let body = r#"[[["مرحبا. ","Hello. ",null,null,10],["كيف حالك؟","How are you?",null,null,10]],null,"en"]"#;
        assert_eq!(parse(body).as_deref(), Some("مرحبا. كيف حالك؟"));
    }

    #[test]
    fn rejects_anything_else_without_panicking() {
        for body in ["", "null", "[]", "[[]]", "[[[1]]]", "{\"a\":1}", "[[[\"  \"]]]", "not json"] {
            assert_eq!(parse(body), None, "{body}");
        }
    }
}
