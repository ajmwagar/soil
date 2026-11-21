// SPDX-License-Identifier: MIT OR Apache-2.0
use soil_lang::sexp::{Value, parse_document, print_document};

const SOURCE: &str = r#"; leading comment
(defun greet (name)
  (list 'hello name))

; second form
(defun answer () 42)
"#;

#[test]
fn keeps_top_level_spans_comments_and_stable_ids() {
    let document = parse_document(SOURCE).unwrap();
    assert_eq!(document.forms.len(), 2);
    assert_eq!(document.forms[0].id, "defun:greet");
    assert_eq!(document.forms[1].id, "defun:answer");
    assert_eq!(document.forms[0].comments[0].text, "; leading comment");
    assert_eq!(document.forms[1].comments[0].text, "; second form");
    assert_eq!(
        &SOURCE[document.forms[0].span.start..document.forms[0].span.end],
        "(defun greet (name)\n  (list 'hello name))"
    );
    assert_eq!(document.print_original(), SOURCE);
}

#[test]
fn canonical_print_round_trips_forms_and_comments() {
    let document = parse_document(SOURCE).unwrap();
    let printed = print_document(&document);
    let reparsed = parse_document(&printed).unwrap();
    assert_eq!(reparsed.forms.len(), document.forms.len());
    assert_eq!(reparsed.forms[0].id, document.forms[0].id);
    assert_eq!(reparsed.forms[1].id, document.forms[1].id);
    assert!(printed.contains("; leading comment"));
    assert!(printed.contains("; second form"));
    assert_eq!(
        reparsed.forms[0].value,
        Value::List(vec![
            Value::Symbol("defun".into()),
            Value::Symbol("greet".into()),
            Value::List(vec![Value::Symbol("name".into())]),
            Value::List(vec![
                Value::Symbol("list".into()),
                Value::List(vec![
                    Value::Symbol("quote".into()),
                    Value::Symbol("hello".into())
                ]),
                Value::Symbol("name".into())
            ])
        ])
    );
}
