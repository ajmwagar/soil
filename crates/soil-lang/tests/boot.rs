// SPDX-License-Identifier: MIT OR Apache-2.0
use std::path::Path;

use soil_lang::Runtime;
use soil_lang::sexp::Value;

fn runtime() -> Runtime {
    let mut runtime = Runtime::new(Box::new(soil_lang::FakeHost::new()));
    runtime
        .load_file(Path::new(env!("CARGO_MANIFEST_DIR")).join("boot/boot.soil"))
        .unwrap();
    runtime
}

fn eval(source: &str) -> Value {
    runtime().load(source).unwrap()
}

#[test]
fn bootstrap_evaluates_atoms_arithmetic_and_if() {
    assert_eq!(eval("(boot/eval '(+ 1 2) '())"), Value::Int(3));
    assert_eq!(
        eval("(boot/eval '(if (= 1 1) 'yes 'no) '())"),
        Value::Symbol("yes".to_string())
    );
}

#[test]
fn bootstrap_binds_and_applies_lambdas() {
    assert_eq!(
        eval("(boot/eval '((lambda (x) (+ x 1)) 4) '())"),
        Value::Int(5)
    );
}

#[test]
fn bootstrap_handles_let_and_cond() {
    assert_eq!(
        eval("(boot/eval '(let ((x 2) (y 3)) (* x y)) '())"),
        Value::Int(6)
    );
    assert_eq!(
        eval("(boot/eval '(cond ((= 1 2) 'no) (else 'yes)) '())"),
        Value::Symbol("yes".to_string())
    );
}

#[test]
fn bootstrap_reads_environment_and_lists() {
    assert_eq!(eval("(boot/eval 'x '((x 5)))"), Value::Int(5));
    assert_eq!(
        eval("(boot/eval '(car (cons 'a '())) '())"),
        Value::Symbol("a".to_string())
    );
}
