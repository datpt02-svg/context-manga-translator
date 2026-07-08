//! OCR quality assessment. Flags suspicious OCR results so downstream
//! systems (or the user) can decide whether to re-run with a different
//! OCR engine or trigger Unlimited-OCR.

// ---------------------------------------------------------------------------
// Constants — ruling thresholds
// ---------------------------------------------------------------------------

const LOW_DETECTOR_CONFIDENCE: f32 = 0.45;
const LOW_OCR_CONFIDENCE: f32 = 0.65;
const LARGE_BOX_AREA: f32 = 20_000.0;
const MIN_JP_RATIO_FOR_LONG_TEXT: f32 = 0.35;

/// Short manga utterances that are legitimately 1-2 chars and should *not*
/// be flagged as suspicious despite being short in a large box.
const COMMON_MANGA_SHORT: &[&str] = &["え？", "うん", "はい", "いや", "あ", "ん？", "…", "！？"];

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct OcrQualityInput<'a> {
    pub text: Option<&'a str>,
    pub detector_confidence: f32,
    pub ocr_confidence: Option<f32>,
    pub bbox_width: f32,
    pub bbox_height: f32,
    pub is_vertical: bool,
}

#[derive(Debug, Clone)]
pub struct OcrQualityReport {
    pub score: f32,
    pub uncertain: bool,
    pub reasons: Vec<OcrQualityReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrQualityReason {
    EmptyText,
    LowDetectorConfidence,
    LowOcrConfidence,
    BadCharacters,
    TooShortForLargeBox,
    LowJapaneseRatio,
    WeirdRepetition,
}

// ---------------------------------------------------------------------------
// Assessor
// ---------------------------------------------------------------------------

/// Evaluate OCR quality and return a report.
///
/// The `uncertain` flag is set when any reason triggers. Callers can inspect
/// `reasons` for fine-grained decisions.
pub fn assess_ocr_quality(input: OcrQualityInput<'_>) -> OcrQualityReport {
    let mut uncertain = false;
    let mut reasons = Vec::new();
    let mut score: f32 = 1.0;

    // 1. Empty text
    let text = match input.text {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => {
            uncertain = true;
            reasons.push(OcrQualityReason::EmptyText);
            score = 0.0;
            return OcrQualityReport {
                score,
                uncertain,
                reasons,
            };
        }
    };

    // 2. Bad characters
    if text.contains('□') || text.contains('�') {
        uncertain = true;
        reasons.push(OcrQualityReason::BadCharacters);
        score *= 0.3;
    }

    // 3. Low detector confidence
    if input.detector_confidence < LOW_DETECTOR_CONFIDENCE {
        uncertain = true;
        reasons.push(OcrQualityReason::LowDetectorConfidence);
        score *= 0.5;
    }

    // 4. Low OCR confidence
    if let Some(ocr_conf) = input.ocr_confidence {
        if ocr_conf < LOW_OCR_CONFIDENCE {
            uncertain = true;
            reasons.push(OcrQualityReason::LowOcrConfidence);
            score *= 0.5;
        }
    }

    // 5. Large bbox with very short text — skip common manga short forms
    let area = input.bbox_width * input.bbox_height;
    if area >= LARGE_BOX_AREA && text.chars().count() <= 2 {
        if !COMMON_MANGA_SHORT.contains(&text) {
            uncertain = true;
            reasons.push(OcrQualityReason::TooShortForLargeBox);
            score *= 0.4;
        } else {
            // Still a slight reduction, but not flagged uncertain.
            score *= 0.9;
        }
    }

    // 6. Low Japanese ratio for longer text
    if text.chars().count() >= 4 {
        let jp_ratio = japanese_ratio(text);
        if jp_ratio < MIN_JP_RATIO_FOR_LONG_TEXT {
            uncertain = true;
            reasons.push(OcrQualityReason::LowJapaneseRatio);
            score *= 0.5;
        }
    }

    // 7. Weird repetition — repeated single char 4+ times (e.g. "ああああ")
    if has_weird_repetition(text) {
        uncertain = true;
        reasons.push(OcrQualityReason::WeirdRepetition);
        score *= 0.3;
    }

    OcrQualityReport {
        score: score.max(0.0),
        uncertain,
        reasons,
    }
}

// ---------------------------------------------------------------------------
// Heuristic helpers
// ---------------------------------------------------------------------------

/// Heuristic: ratio of characters that look like Japanese (Hiragana, Katakana,
/// CJK ideographs).
fn japanese_ratio(text: &str) -> f32 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let jp = text
        .chars()
        .filter(|&c| {
            matches!(c,
                '\u{3040}'..='\u{309F}'  // Hiragana
                | '\u{30A0}'..='\u{30FF}'  // Katakana
                | '\u{3400}'..='\u{4DBF}'  // CJK Extension A
                | '\u{4E00}'..='\u{9FFF}'  // CJK Unified
                | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility
            )
        })
        .count();
    jp as f32 / total as f32
}

/// Check for repeated single character 4+ times (e.g. "ああああ", "!!!!").
/// Skips common patterns like "………" (single char repeated but valid).
fn has_weird_repetition(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    // Repeated same char 4+ times (only one distinct char in the string)
    let unique: std::collections::HashSet<char> = text.chars().collect();
    if unique.len() == 1 && text.chars().count() >= 4 {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_input() -> OcrQualityInput<'static> {
        OcrQualityInput {
            text: Some("これはテストです"),
            detector_confidence: 0.95,
            ocr_confidence: Some(0.95),
            bbox_width: 200.0,
            bbox_height: 50.0,
            is_vertical: false,
        }
    }

    #[test]
    fn empty_text_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            text: Some(""),
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::EmptyText));
    }

    #[test]
    fn none_text_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            text: None,
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::EmptyText));
    }

    #[test]
    fn good_japanese_text_not_uncertain() {
        let report = assess_ocr_quality(default_input());
        assert!(!report.uncertain);
    }

    #[test]
    fn bad_chars_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            text: Some("テスト□"),
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::BadCharacters));
    }

    #[test]
    fn replacement_char_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            text: Some("テス�ト"),
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::BadCharacters));
    }

    #[test]
    fn low_detector_confidence_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            detector_confidence: 0.3,
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::LowDetectorConfidence));
    }

    #[test]
    fn low_ocr_confidence_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            ocr_confidence: Some(0.5),
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::LowOcrConfidence));
    }

    #[test]
    fn large_bbox_short_text_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            text: Some("ab"),
            bbox_width: 300.0,
            bbox_height: 200.0,
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::TooShortForLargeBox));
    }

    #[test]
    fn common_manga_short_not_uncertain() {
        for &utterance in COMMON_MANGA_SHORT {
            let report = assess_ocr_quality(OcrQualityInput {
                text: Some(utterance),
                bbox_width: 300.0,
                bbox_height: 200.0,
                ..default_input()
            });
            assert!(
                !report.uncertain,
                "{utterance} should not be uncertain in large box"
            );
        }
    }

    #[test]
    fn low_japanese_ratio_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            text: Some("Hello World 12345"),
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::LowJapaneseRatio));
    }

    #[test]
    fn weird_repetition_uncertain() {
        let report = assess_ocr_quality(OcrQualityInput {
            text: Some("ああああ"),
            ..default_input()
        });
        assert!(report.uncertain);
        assert!(report.reasons.contains(&OcrQualityReason::WeirdRepetition));
    }

    #[test]
    fn japanese_ratio_all_jp() {
        assert_eq!(japanese_ratio("こんにちは"), 1.0);
    }

    #[test]
    fn japanese_ratio_mixed() {
        let r = japanese_ratio("Helloこんにちは");
        assert!((r - 0.5).abs() < 0.01);
    }

    #[test]
    fn japanese_ratio_empty() {
        assert_eq!(japanese_ratio(""), 0.0);
    }
}
