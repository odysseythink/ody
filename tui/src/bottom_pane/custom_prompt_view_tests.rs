use super::*;
use pretty_assertions::assert_eq;
use std::sync::mpsc::Receiver;

#[test]
fn paste_burst_newline_does_not_submit_short_first_line() {
    let now = Instant::now();

    for (first_line, second_line) in [("x", "rest"), ("id", "body"), ("foo", "bar")] {
        let (mut view, submitted_rx) = custom_prompt_view();
        let mut ms = 0;

        for ch in first_line.chars() {
            view.handle_key_event_at(KeyEvent::from(KeyCode::Char(ch)), now + elapsed(ms));
            ms += 1;
        }
        view.handle_key_event_at(KeyEvent::from(KeyCode::Enter), now + elapsed(ms));
        ms += 1;
        for ch in second_line.chars() {
            view.handle_key_event_at(KeyEvent::from(KeyCode::Char(ch)), now + elapsed(ms));
            ms += 1;
        }

        assert!(submitted_rx.try_recv().is_err());
        assert!(!view.is_complete());

        view.handle_key_event_at(KeyEvent::from(KeyCode::Enter), now + elapsed(/*ms*/ 200));

        assert_eq!(
            submitted_rx.try_recv(),
            Ok(format!("{first_line}\n{second_line}"))
        );
        assert!(view.is_complete());
    }
}

#[test]
fn paste_burst_newline_after_tab_does_not_submit() {
    let (mut view, submitted_rx) = custom_prompt_view();
    let now = Instant::now();
    let mut ms = 0;

    view.handle_key_event_at(KeyEvent::from(KeyCode::Char('x')), now + elapsed(ms));
    ms += 1;
    view.handle_key_event_at(KeyEvent::from(KeyCode::Tab), now + elapsed(ms));
    ms += 1;
    view.handle_key_event_at(KeyEvent::from(KeyCode::Enter), now + elapsed(ms));
    ms += 1;
    for ch in "rest".chars() {
        view.handle_key_event_at(KeyEvent::from(KeyCode::Char(ch)), now + elapsed(ms));
        ms += 1;
    }

    assert!(submitted_rx.try_recv().is_err());
    assert!(!view.is_complete());

    view.handle_key_event_at(KeyEvent::from(KeyCode::Enter), now + elapsed(/*ms*/ 200));

    assert_eq!(submitted_rx.try_recv(), Ok("x\nrest".to_string()));
    assert!(view.is_complete());
}

#[test]
fn delayed_enter_after_typing_submits() {
    let (mut view, submitted_rx) = custom_prompt_view();
    let now = Instant::now();

    for (idx, ch) in "foo".chars().enumerate() {
        view.handle_key_event_at(KeyEvent::from(KeyCode::Char(ch)), now + elapsed(idx * 20));
    }
    view.handle_key_event_at(KeyEvent::from(KeyCode::Enter), now + elapsed(/*ms*/ 80));

    assert_eq!(submitted_rx.try_recv(), Ok("foo".to_string()));
    assert!(view.is_complete());
}

fn custom_prompt_view() -> (CustomPromptView, Receiver<String>) {
    let (submitted, submitted_rx) = std::sync::mpsc::channel();
    let view = CustomPromptView::new(
        "Edit goal".to_string(),
        "Type a goal objective and press Enter".to_string(),
        String::new(),
        /*context_label*/ None,
        Box::new(move |text| {
            submitted.send(text).expect("send submitted text");
        }),
    );
    (view, submitted_rx)
}

fn elapsed(ms: usize) -> std::time::Duration {
    std::time::Duration::from_millis(ms as u64)
}

#[test]
fn new_secret_prompt_masks_input() {
    let (view, _submitted_rx) = custom_prompt_view();
    assert!(!view.secret);

    let (secret_view, _submitted_rx) = custom_prompt_view_secret();
    assert!(secret_view.secret);
}

#[test]
fn secret_prompt_submits_masked_text() {
    let (mut view, submitted_rx) = custom_prompt_view_secret();

    view.handle_paste("secret-api-key".to_string());
    view.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_eq!(submitted_rx.try_recv(), Ok("secret-api-key".to_string()));
    assert!(view.is_complete());
}

#[test]
fn secret_rendering_masks_characters() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    let (mut view, _submitted_rx) = custom_prompt_view_secret();
    view.handle_paste("abc".to_string());

    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
    view.render(buf.area, &mut buf);

    let rendered = buf
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(
        rendered.contains('*'),
        "expected rendered output to contain masked characters, got: {rendered}"
    );
    assert!(
        !rendered.contains("abc"),
        "expected rendered output not to contain the raw secret, got: {rendered}"
    );
}

fn custom_prompt_view_secret() -> (CustomPromptView, Receiver<String>) {
    let (submitted, submitted_rx) = std::sync::mpsc::channel();
    let view = CustomPromptView::new_secret(
        "Enter API key".to_string(),
        "Paste your key".to_string(),
        String::new(),
        /*context_label*/ None,
        Box::new(move |text| {
            submitted.send(text).expect("send submitted text");
        }),
    );
    (view, submitted_rx)
}
