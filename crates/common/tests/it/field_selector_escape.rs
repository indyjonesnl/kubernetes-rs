//! Ports of `apimachinery/pkg/fields/selector_test.go`: TestSplitTerms,
//! TestSplitTerm, TestEscapeValue, TestSelectorParse, TestDeterministicParse.

use kubernetes_rs_common::field_selector::*;

#[test]
fn field_selector_escape_split_terms() {
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("a", vec!["a"]),
        ("a=avalue", vec!["a=avalue"]),
        ("a=avalue,b=bvalue", vec!["a=avalue", "b=bvalue"]),
        (
            "a=avalue,b==bvalue,c!=cvalue",
            vec!["a=avalue", "b==bvalue", "c!=cvalue"],
        ),
        ("", vec![]),
        ("a=a,", vec!["a=a", ""]),
        (",a=a", vec!["", "a=a"]),
        (r"k=\,,k2=v2", vec![r"k=\,", "k2=v2"]),
        (r"k=\\,k2=v2", vec![r"k=\\", "k2=v2"]),
        (r"k=\\\,,k2=v2", vec![r"k=\\\,", "k2=v2"]),
        (r"k=\a\b\", vec![r"k=\a\b\"]),
        (r"k=\", vec![r"k=\"]),
        ("함=수,목=록", vec!["함=수", "목=록"]),
    ];
    for (sel, want) in cases {
        assert_eq!(split_terms(sel), want, "{sel}");
    }
}

#[test]
fn field_selector_escape_split_term() {
    type Split<'a> = Option<(&'a str, &'a str, &'a str)>;
    let cases: Vec<(&str, Split)> = vec![
        ("a=value", Some(("a", "=", "value"))),
        ("b==value", Some(("b", "==", "value"))),
        ("c!=value", Some(("c", "!=", "value"))),
        ("", None),
        ("a", None),
        (r"k=\,", Some(("k", "=", r"\,"))),
        (r"k=\=", Some(("k", "=", r"\="))),
        (r"k=\\\a\b\=\,\", Some(("k", "=", r"\\\a\b\=\,\"))),
        ("함=수", Some(("함", "=", "수"))),
    ];
    for (term, want) in cases {
        assert_eq!(split_term(term), want, "{term}");
    }
}

#[test]
fn field_selector_escape_value_roundtrip_and_invalid() {
    for (raw, esc) in [
        ("", ""),
        ("a", "a"),
        ("=", r"\="),
        (",", r"\,"),
        (r"\", r"\\"),
        (r"\=\,\", r"\\\=\\\,\\"),
    ] {
        assert_eq!(escape_value(raw), esc);
        assert_eq!(unescape_value(esc).unwrap(), raw);
    }
    for bad in [r"\", r"\\\", r"\a"] {
        assert!(
            matches!(
                unescape_value(bad),
                Err(FieldSelectorError::InvalidEscapeSequence(_))
            ),
            "{bad}"
        );
    }
    assert_eq!(
        unescape_value(r"\a").unwrap_err().to_string(),
        r"invalid field selector: invalid escape sequence: \a"
    );
    assert_eq!(
        unescape_value("a=b").unwrap_err().to_string(),
        "invalid field selector: unescaped character in value: 61"
    );
}

#[test]
fn field_selector_escape_selector_parse() {
    for good in ["x=a,y=b,z=c", "", "x!=a,y=b", r"x=a||y\=b", r"x=a\=\=b"] {
        let s = FieldSelector::parse(good).unwrap_or_else(|e| panic!("{good}: {e}"));
        assert_eq!(s.to_string(), good);
    }
    for bad in [
        "x=a||y=b",
        "x==a==b",
        "x=a,b",
        "x in (a)",
        "x in (a,b,c)",
        "x",
    ] {
        assert!(FieldSelector::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(
        FieldSelector::parse("x").unwrap_err().to_string(),
        "invalid selector: 'x'; can't understand 'x'"
    );
}

#[test]
fn field_selector_escape_deterministic_parse() {
    let a = FieldSelector::parse("x=a,a=x").unwrap();
    let b = FieldSelector::parse("a=x,x=a").unwrap();
    assert_eq!(a.to_string(), b.to_string());
}

#[test]
fn field_selector_escape_value_with_comma_matches() {
    let s = FieldSelector::parse(r"metadata.name=a\,b").unwrap();
    assert!(s.matches(&serde_json::json!({"metadata": {"name": "a,b"}})));
}
