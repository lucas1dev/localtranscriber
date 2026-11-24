//! Núcleo de resumo: prompts, divisão de texto e map-reduce (porte de `ai_engine.py`).

use async_trait::async_trait;

use crate::error::AppError;

pub const SYSTEM: &str = "Você é um assistente especializado em análise de transcrições. Trabalhe somente com o conteúdo fornecido. Não invente fatos, nomes, decisões, citações, responsáveis ou timestamps. Diferencie afirmações explícitas de inferências. Preserve nomes próprios e termos técnicos. Quando houver timestamps, associe pontos importantes aos tempos correspondentes. Responda em português brasileiro, salvo pedido contrário. Retorne Markdown claro.";

pub const CHUNK_LIMIT: usize = 22_000;

pub fn style_prompt(style: &str) -> Option<&'static str> {
    Some(match style {
        "executive" => "Crie um resumo executivo conciso, com contexto, ideias centrais e conclusão.",
        "detailed" => "Crie um resumo detalhado, organizado em seções e subtópicos, preservando nuances e argumentos.",
        "academic" => "Produza uma síntese acadêmica: tema, objetivo, conceitos, metodologia (se mencionada), argumentos, evidências e conclusões.",
        "meeting" => "Organize como ata: contexto, assuntos discutidos, decisões explícitas, tarefas, responsáveis e prazos. Não invente responsáveis ou prazos.",
        "class" => "Organize como material de estudo: tema, objetivos, explicações, conceitos-chave, exemplos, dúvidas e revisão final.",
        "podcast" => "Resuma como episódio de podcast: tema, ideias principais, momentos relevantes, opiniões e conclusões.",
        "custom" => "Siga cuidadosamente as instruções personalizadas fornecidas pelo usuário.",
        _ => return None,
    })
}
#[async_trait]
pub trait Llm: Send + Sync {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AppError>;
}

pub fn user_message(prompt: &str, text: &str) -> String {
    format!("{prompt}\n\nTRANSCRIÇÃO:\n{text}")
}

/// Whitespace as Python's `str.strip()` / `str.isspace()` sees it.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Equivalent of Python's `str.splitlines()` (without keeping line ends).
fn py_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut iter = text.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        let is_break = matches!(
            c,
            '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        );
        if !is_break {
            continue;
        }
        lines.push(&text[start..i]);
        start = i + c.len_utf8();
        if c == '\r' {
            if let Some(&(_, '\n')) = iter.peek() {
                iter.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python's `s.rfind(sep, 0, end)` over char indices; `sep` is two chars.
fn rfind_in_prefix(chars: &[char], sep: [char; 2], end: usize) -> Option<usize> {
    let end = end.min(chars.len());
    if end < 2 {
        return None;
    }
    (0..=end - 2).rev().find(|&i| chars[i] == sep[0] && chars[i + 1] == sep[1])
}

fn char_slice(chars: &[char]) -> String {
    chars.iter().collect()
}

/// Faithful port of `_split_text`; lengths are counted in chars.
pub fn split_text(text: &str, limit: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for line in py_splitlines(text) {
        let mut paragraph: Vec<char> = py_strip(line).chars().collect();
        if paragraph.is_empty() {
            continue;
        }
        while paragraph.len() > limit {
            let found = [['.', ' '], ['?', ' '], ['!', ' ']]
                .iter()
                .filter_map(|sep| rfind_in_prefix(&paragraph, *sep, limit))
                .max();
            let cut = match found {
                Some(c) if c >= limit / 2 => c,
                _ => limit,
            };
            let end = (cut + 1).min(paragraph.len());
            chunks.push(py_strip(&char_slice(&paragraph[..end])).to_string());
            let rest = char_slice(&paragraph[end..]);
            paragraph = py_strip(&rest).chars().collect();
        }
        if !current.is_empty() && current_len + paragraph.len() + 1 > limit {
            chunks.push(std::mem::take(&mut current));
            current = char_slice(&paragraph);
        } else {
            current = py_strip(&format!("{current}\n{}", char_slice(&paragraph))).to_string();
        }
        current_len = current.chars().count();
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        vec![text.to_string()]
    } else {
        chunks
    }
}

/// Returns `(summary, chunks_processed)`.
pub async fn run_summary(
    llm: &dyn Llm,
    text: &str,
    style: &str,
    custom_instructions: &str,
) -> Result<(String, usize), AppError> {
    if text.trim().is_empty() {
        return Err(AppError::User("A transcrição está vazia".into()));
    }
    let base = style_prompt(style)
        .ok_or_else(|| AppError::User("Formato de resumo inválido".into()))?;
    let mut prompt = base.to_string();
    if style == "custom" && !custom_instructions.trim().is_empty() {
        prompt.push_str("\nInstruções do usuário: ");
        prompt.push_str(custom_instructions.trim());
    }
    let chunks = split_text(text, CHUNK_LIMIT);
    let n = chunks.len();
    if n == 1 {
        let result = llm.complete(SYSTEM, &user_message(&prompt, &chunks[0])).await?;
        return Ok((result, 1));
    }
    let mut partials = Vec::with_capacity(n);
    for (i, chunk) in chunks.iter().enumerate() {
        let index = i + 1;
        let partial_prompt = format!(
            "Resuma este trecho mantendo fatos, nomes, conceitos e referências temporais. Não conclua o conteúdo global ainda. Identifique como trecho {index} de {n}.\n{prompt}"
        );
        let partial = llm
            .complete(SYSTEM, &user_message(&partial_prompt, chunk))
            .await?;
        partials.push(format!("TRECHO {index}\n{partial}"));
    }
    let consolidation = format!(
        "Consolide as sínteses parciais em um único resultado coerente, elimine repetições sem perder informações importantes e sinalize divergências. {prompt}"
    );
    let result = llm
        .complete(SYSTEM, &user_message(&consolidation, &partials.join("\n\n")))
        .await?;
    Ok((result, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeLlm {
        calls: Mutex<Vec<(String, String)>>,
    }

    impl FakeLlm {
        fn new() -> Self {
            Self { calls: Mutex::new(Vec::new()) }
        }
    }

    #[async_trait]
    impl Llm for FakeLlm {
        async fn complete(&self, system: &str, user: &str) -> Result<String, AppError> {
            let mut calls = self.calls.lock().unwrap();
            calls.push((system.to_string(), user.to_string()));
            Ok(format!("R{}", calls.len()))
        }
    }

    #[test]
    fn split_short_text_single_chunk() {
        assert_eq!(split_text("a\nb", 22_000), vec!["a\nb"]);
    }

    #[test]
    fn split_counts_chars_not_bytes() {
        let t = "ã".repeat(30);
        let chunks = split_text(&t, 20);
        assert_eq!(chunks.iter().map(|c| c.chars().count()).sum::<usize>(), 30);
        // Python: [21, 9] (hard cut yields limit+1 chars)
        assert_eq!(chunks.iter().map(|c| c.chars().count()).collect::<Vec<_>>(), vec![21, 9]);
    }

    #[test]
    fn split_prefers_sentence_boundary() {
        let t = format!("{}. {}", "a".repeat(15), "b".repeat(15));
        let c = split_text(&t, 20);
        assert_eq!(c[0], format!("{}.", "a".repeat(15)));
        assert_eq!(c[1], "b".repeat(15));
    }

    #[test]
    fn split_drops_blank_lines() {
        assert_eq!(split_text("a\n\n\nb", 100), vec!["a\nb"]);
    }

    #[test]
    fn split_hard_cut_matches_python() {
        // Python: ['abcdefghija', 'bcdefghijab', 'cdefghij kk', 'kkk']
        let t = format!("{} kkkkk", "abcdefghij".repeat(3));
        assert_eq!(
            split_text(&t, 10),
            vec!["abcdefghija", "bcdefghijab", "cdefghij kk", "kkk"]
        );
        // Python: [21, 21, 3]
        let lens: Vec<usize> = split_text(&"z".repeat(45), 20).iter().map(|c| c.len()).collect();
        assert_eq!(lens, vec![21, 21, 3]);
    }

    #[test]
    fn split_blank_text_returns_original() {
        assert_eq!(split_text("   ", 100), vec!["   "]);
    }

    #[test]
    fn split_packs_paragraphs_by_limit() {
        // Python: [21999, 21999, 5999]
        let t = vec!["p".repeat(999); 50].join("\n");
        let lens: Vec<usize> = split_text(&t, CHUNK_LIMIT).iter().map(|c| c.len()).collect();
        assert_eq!(lens, vec![21999, 21999, 5999]);
    }

    #[tokio::test]
    async fn single_chunk_one_call() {
        let llm = FakeLlm::new();
        let (out, n) = run_summary(&llm, "texto curto", "executive", "").await.unwrap();
        assert_eq!((out.as_str(), n), ("R1", 1));
        let calls = llm.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, SYSTEM);
        assert_eq!(
            calls[0].1,
            format!("{}\n\nTRANSCRIÇÃO:\ntexto curto", style_prompt("executive").unwrap())
        );
    }

    #[tokio::test]
    async fn map_reduce_calls_n_plus_one() {
        let text = vec!["q".repeat(1_000); 50].join("\n");
        let llm = FakeLlm::new();
        let (out, n) = run_summary(&llm, &text, "executive", "").await.unwrap();
        assert_eq!(n, 3);
        assert_eq!(out, "R4");
        let calls = llm.calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert!(calls[0].1.starts_with(
            "Resuma este trecho mantendo fatos, nomes, conceitos e referências temporais. Não conclua o conteúdo global ainda. Identifique como trecho 1 de 3.\n"
        ));
        assert!(calls[3].1.starts_with("Consolide as sínteses parciais"));
        assert!(calls[3].1.ends_with("TRECHO 1\nR1\n\nTRECHO 2\nR2\n\nTRECHO 3\nR3"));
    }

    #[tokio::test]
    async fn custom_instructions_appended_only_for_custom() {
        let llm = FakeLlm::new();
        run_summary(&llm, "texto", "custom", "foco em prazos").await.unwrap();
        assert!(llm.calls.lock().unwrap()[0].1.contains("Instruções do usuário: foco em prazos"));
        let llm = FakeLlm::new();
        run_summary(&llm, "texto", "executive", "foco em prazos").await.unwrap();
        assert!(!llm.calls.lock().unwrap()[0].1.contains("Instruções do usuário"));
    }

    #[tokio::test]
    async fn empty_text_is_user_error() {
        let err = run_summary(&FakeLlm::new(), "   ", "executive", "").await.unwrap_err();
        assert!(matches!(err, AppError::User(m) if m == "A transcrição está vazia"));
    }

    #[tokio::test]
    async fn invalid_style_is_user_error() {
        let err = run_summary(&FakeLlm::new(), "texto", "xyz", "").await.unwrap_err();
        assert!(matches!(err, AppError::User(m) if m == "Formato de resumo inválido"));
    }
}
