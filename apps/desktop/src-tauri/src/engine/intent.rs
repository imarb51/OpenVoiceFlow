use super::types::Application;

pub enum Route {
    Dictate,
    LaunchApp(Application),
    Reasoning,
}

/// Keep common speech out of the paid reasoning call. Clear, exact app-launch
/// requests resolve against the local Windows application catalog.
pub fn route(transcript: &str, apps: &[Application]) -> Route {
    let command = strip_polite_prefix(&normalize(transcript));
    if let Some(query) = app_launch_target(&command) {
        if let Some(app) = find_exact_app(&query, apps) {
            return Route::LaunchApp(app.clone());
        }
    }
    if starts_with_command_verb(&command) {
        Route::Reasoning
    } else {
        Route::Dictate
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '+' || c == '#' {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_polite_prefix(text: &str) -> String {
    let mut value = text.trim().to_string();
    let prefixes = [
        "could you please ",
        "would you please ",
        "can you please ",
        "could you ",
        "would you ",
        "can you ",
        "will you ",
        "please ",
        "hey there ",
        "hello there ",
        "hey ",
        "hello ",
        "hi there ",
        "hi ",
    ];
    loop {
        let Some(prefix) = prefixes.iter().find(|prefix| value.starts_with(**prefix)) else {
            break;
        };
        value = value[prefix.len()..].trim().to_string();
    }
    value
}

fn app_launch_target(text: &str) -> Option<String> {
    let rest = ["open ", "launch ", "start ", "run "]
        .iter()
        .find_map(|verb| text.strip_prefix(verb))?;
    let mut words = rest.split_whitespace().collect::<Vec<_>>();
    while words
        .last()
        .is_some_and(|word| ["please", "now", "for", "me", "app"].contains(word))
    {
        words.pop();
    }
    if words.first() == Some(&"the") {
        words.remove(0);
    }
    if words.last() == Some(&"for") {
        words.pop();
    }
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

fn starts_with_command_verb(text: &str) -> bool {
    [
        "open ",
        "launch ",
        "start ",
        "run ",
        "close ",
        "quit ",
        "click ",
        "press ",
        "type ",
        "search ",
        "find ",
        "navigate ",
        "switch ",
        "select ",
        "scroll ",
        "play ",
        "pause ",
        "send ",
        "delete ",
        "save ",
        "create ",
        "go to ",
    ]
    .iter()
    .any(|verb| text.starts_with(verb))
}

fn normalized_app_name(text: &str) -> String {
    let normalized = normalize(text);
    normalized
        .strip_suffix(" exe")
        .or_else(|| normalized.strip_suffix(".exe"))
        .unwrap_or(&normalized)
        .to_string()
}

fn find_exact_app<'a>(query: &str, apps: &'a [Application]) -> Option<&'a Application> {
    let query = normalized_app_name(query);
    let score = |app: &Application| {
        let raw_name = normalize(&app.name);
        let name = normalized_app_name(&app.name);
        let compact_query = query.replace(' ', "");
        let executable = std::path::Path::new(&app.executable)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(normalized_app_name)
            .unwrap_or_default();
        if raw_name == query {
            Some(0)
        } else if !raw_name.ends_with(".exe") && name.replace(' ', "") == compact_query {
            Some(1)
        } else if name == query || name.replace(' ', "") == compact_query {
            Some(2)
        } else if executable == query {
            Some(3)
        } else if query.chars().count() >= 6
            && one_edit_apart(&query, &name)
            && !raw_name.ends_with(".exe")
        {
            Some(4)
        } else if query.chars().count() >= 6 && one_edit_apart(&query, &name) {
            Some(5)
        } else if query.chars().count() >= 6 && one_edit_apart(&query, &executable) {
            Some(6)
        } else {
            None
        }
    };
    let mut candidates = apps
        .iter()
        .filter_map(|app| score(app).map(|score| (score, app)))
        .collect::<Vec<_>>();
    let Some(best_score) = candidates.iter().map(|(score, _)| *score).min() else {
        return None;
    };
    candidates.retain(|(score, _)| *score == best_score);
    (candidates.len() == 1).then(|| candidates[0].1)
}

fn one_edit_apart(left: &str, right: &str) -> bool {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < left.len() && j < right.len() {
        if left[i] == right[j] {
            i += 1;
            j += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if left.len() == right.len() {
                i += 1;
                j += 1;
            } else if left.len() > right.len() {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    edits + usize::from(i < left.len() || j < right.len()) <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, executable: &str) -> Application {
        Application {
            id: format!("app-{}", name.to_lowercase()),
            name: name.into(),
            executable: executable.into(),
            arguments: vec![],
        }
    }

    #[test]
    fn exact_open_app_request_uses_local_catalog() {
        let apps = vec![
            app("Notepad", "explorer.exe"),
            app("notepad.exe", r"C:\Program Files\WindowsApps\Notepad.exe"),
        ];
        assert!(
            matches!(route("Hey there, open Notepad please", &apps), Route::LaunchApp(a) if a.name == "Notepad")
        );
        assert!(
            matches!(route("Can you launch notepad.exe?", &apps), Route::LaunchApp(a) if a.name == "Notepad")
        );
        assert!(
            matches!(route("Open the note pad app", &apps), Route::LaunchApp(a) if a.name == "Notepad")
        );
        assert!(
            matches!(route("Can you open nodepad?", &apps), Route::LaunchApp(a) if a.name == "Notepad")
        );
    }

    #[test]
    fn ordinary_speech_stays_on_the_direct_dictation_path() {
        assert!(matches!(
            route("Hi there, can you tell me what is happening?", &[]),
            Route::Dictate
        ));
        assert!(matches!(
            route("I want to open the discussion", &[]),
            Route::Dictate
        ));
    }

    #[test]
    fn complex_commands_use_the_reasoning_engine() {
        assert!(matches!(
            route("Open Notepad and type hello", &[]),
            Route::Reasoning
        ));
        assert!(matches!(
            route("Click the Save button", &[]),
            Route::Reasoning
        ));
    }

    #[test]
    fn ambiguous_app_names_do_not_pick_arbitrarily() {
        let apps = vec![
            app("Notepad", r"C:\Windows\System32\notepad.exe"),
            app("Notepad", r"C:\Program Files\Notepad\notepad.exe"),
        ];
        assert!(matches!(route("Open Notepad", &apps), Route::Reasoning));
    }
}
