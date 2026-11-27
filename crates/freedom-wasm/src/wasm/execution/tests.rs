use super::{
    ExecutionError, FunctionError, execute_function, execute_function_with_fuel, validate_function,
};
use crate::wasm::test_support::{expression, module};
use crate::wasm::{
    BinaryErrorKind,
    binary::scan_module,
    code::{FunctionBody, LocalGroup, decode_bodies, link_functions},
    decode::{Declarations, FunctionType, ValueType, decode_declarations},
};

#[test]
fn decodes_links_checks_and_executes_a_binary_function() {
    let bytes = module(r#"(module (func (export "f") (result i32) i32.const 42))"#);
    let module = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&module).unwrap();
    let bodies = decode_bodies(&module).unwrap();
    let linked = link_functions(&declarations, &bodies).unwrap();
    let (type_index, body) = linked.iter().next().unwrap();
    let function = validate_function(&declarations, type_index, body).unwrap();
    assert_eq!(execute_function(&function, &[]).unwrap(), Some(42));
}

#[test]
fn checks_result_stack_and_reports_unsupported_features() {
    let declarations = Declarations {
        types: vec![
            FunctionType {
                parameters: vec![],
                results: vec![ValueType::I32],
            },
            FunctionType {
                parameters: vec![],
                results: vec![],
            },
            FunctionType {
                parameters: vec![ValueType::I64],
                results: vec![ValueType::I32],
            },
        ],
        function_types: vec![],
    };
    let body = |instructions| FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions,
    };
    let minus_one_instructions = expression("i32.const -1");
    let minus_one = body(&minus_one_instructions);
    assert_eq!(
        execute_function(
            &validate_function(&declarations, 0, &minus_one).unwrap(),
            &[]
        )
        .unwrap(),
        Some(-1)
    );
    let empty_instructions = expression("");
    let empty = body(&empty_instructions);
    assert_eq!(
        execute_function(&validate_function(&declarations, 1, &empty).unwrap(), &[]).unwrap(),
        None
    );
    assert!(matches!(
        validate_function(&declarations, 3, &empty),
        Err(FunctionError::UnknownTypeIndex(3))
    ));
    assert!(matches!(
        validate_function(&declarations, 2, &empty),
        Err(FunctionError::UnsupportedSignature)
    ));
    assert!(matches!(
        validate_function(&declarations, 0, &empty),
        Err(FunctionError::ResultStackMismatch {
            expected: 1,
            actual: 0,
            offset: 20,
        })
    ));
    let excess_instructions = expression("i32.const 1 i32.const 2");
    let excess = body(&excess_instructions);
    assert!(matches!(
        validate_function(&declarations, 0, &excess),
        Err(FunctionError::ResultStackMismatch {
            expected: 1,
            actual: 2,
            offset: 24,
        })
    ));
    let unsupported = body(&[0x79, 0x0b]);
    assert!(matches!(
        validate_function(&declarations, 0, &unsupported),
        Err(FunctionError::Decode(error)) if error.kind == BinaryErrorKind::UnsupportedOpcode(0x79)
    ));
    let local_body = FunctionBody {
        locals: vec![LocalGroup {
            count: 1,
            value_type: ValueType::I64,
        }],
        ..empty
    };
    assert!(matches!(
        validate_function(&declarations, 1, &local_body),
        Err(FunctionError::UnsupportedLocals)
    ));
}

#[test]
fn executes_addition_and_rejects_stack_underflow() {
    let bytes = module(
        r#"(module (func (export "f") (result i32)
            i32.const 20 i32.const 22 i32.add))"#,
    );
    let module = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&module).unwrap();
    let bodies = decode_bodies(&module).unwrap();
    let linked = link_functions(&declarations, &bodies).unwrap();
    let (type_index, body) = linked.iter().next().unwrap();
    let function = validate_function(&declarations, type_index, body).unwrap();
    assert_eq!(execute_function(&function, &[]).unwrap(), Some(42));

    let overflow_instructions = expression("i32.const 2147483647 i32.const 1 i32.add");
    let overflow = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &overflow_instructions,
    };
    assert_eq!(
        execute_function(
            &validate_function(&declarations, 0, &overflow).unwrap(),
            &[]
        )
        .unwrap(),
        Some(i32::MIN)
    );
    let underflow_instructions = expression("i32.const 1 i32.add");
    let underflow = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &underflow_instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &underflow),
        Err(FunctionError::StackUnderflow {
            offset: 22,
            needed: 2,
            available: 1
        })
    ));
}

#[test]
fn executes_i32_arithmetic_from_binary_and_reports_traps() {
    let template =
        module("(module (func (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))");
    let cases = [
        ("i32.add", i32::MAX, 1, Ok(i32::MIN)),
        ("i32.sub", 3, 7, Ok(-4)),
        ("i32.sub", i32::MIN, 1, Ok(i32::MAX)),
        ("i32.mul", i32::MAX, 2, Ok(-2)),
        ("i32.div_s", -7, 2, Ok(-3)),
        ("i32.div_u", -1, 2, Ok(i32::MAX)),
        ("i32.rem_s", -7, 2, Ok(-1)),
        ("i32.rem_s", i32::MIN, -1, Ok(0)),
        ("i32.rem_u", -1, 2, Ok(1)),
        (
            "i32.div_s",
            i32::MIN,
            -1,
            Err(ExecutionError::IntegerOverflow {
                offset: template.len() - 2,
            }),
        ),
        (
            "i32.div_s",
            1,
            0,
            Err(ExecutionError::IntegerDivideByZero {
                offset: template.len() - 2,
            }),
        ),
        (
            "i32.div_u",
            1,
            0,
            Err(ExecutionError::IntegerDivideByZero {
                offset: template.len() - 2,
            }),
        ),
        (
            "i32.rem_s",
            1,
            0,
            Err(ExecutionError::IntegerDivideByZero {
                offset: template.len() - 2,
            }),
        ),
        (
            "i32.rem_u",
            1,
            0,
            Err(ExecutionError::IntegerDivideByZero {
                offset: template.len() - 2,
            }),
        ),
    ];
    for (opcode, left, right, expected) in cases {
        let bytes = module(&format!(
            "(module (func (param i32 i32) (result i32) local.get 0 local.get 1 {opcode}))",
        ));
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (type_index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, type_index, body).unwrap();
        assert_eq!(
            execute_function(&function, &[left, right]),
            expected.map(Some),
            "instruciton {opcode}, operands {left}, {right}"
        );

        let instructions = expression(&format!("local.get 0 {opcode}"));
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(matches!(
            validate_function(&declarations, type_index, &body),
            Err(FunctionError::StackUnderflow {
                offset: 22,
                needed: 2,
                available: 1,
            })
        ));
    }
}

#[test]
fn executes_i32_comparisons_and_checks_stack_arity() {
    // Expected results for (-1, 1), (5, 5), and (1, -1), respectively.
    let cases = [
        ("i32.eq", 0x46, [0, 1, 0]),
        ("i32.ne", 0x47, [1, 0, 1]),
        ("i32.lt_s", 0x48, [1, 0, 0]),
        ("i32.lt_u", 0x49, [0, 0, 1]),
        ("i32.gt_s", 0x4a, [0, 0, 1]),
        ("i32.gt_u", 0x4b, [1, 0, 0]),
        ("i32.le_s", 0x4c, [1, 1, 0]),
        ("i32.le_u", 0x4d, [0, 1, 1]),
        ("i32.ge_s", 0x4e, [0, 1, 1]),
        ("i32.ge_u", 0x4f, [1, 1, 0]),
    ];
    for (op, opcode, expected) in cases {
        assert_eq!(expression(op), [opcode, 0x0b]);
        let bytes = module(&format!(
            "(module (func (param i32 i32) (result i32) local.get 0 local.get 1 {op}))"
        ));
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        for (arguments, result) in [
            ([-1, 1], expected[0]),
            ([5, 5], expected[1]),
            ([1, -1], expected[2]),
        ] {
            assert_eq!(
                execute_function(&function, &arguments),
                Ok(Some(result)),
                "{op} {arguments:?}"
            );
        }
        for (source, available) in [(op.to_owned(), 0), (format!("local.get 0 {op}"), 1)] {
            let instructions = expression(&source);
            let body = FunctionBody {
                locals: vec![],
                instructions_offset: 20,
                instructions: &instructions,
            };
            assert!(matches!(
                validate_function(&declarations, 0, &body),
                Err(FunctionError::StackUnderflow {
                    offset,
                    needed: 2,
                    available: actual
                }) if offset == 20 + available * 2 && actual == available
            ));
        }
    }

    assert_eq!(expression("i32.eqz"), [0x45, 0x0b]);
    let bytes = module("(module (func (param i32) (result i32) local.get 0 i32.eqz))");
    let framed = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&framed).unwrap();
    let bodies = decode_bodies(&framed).unwrap();
    let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
    for (argument, expected) in [(0, 1), (-1, 0), (i32::MIN, 0), (i32::MAX, 0)] {
        assert_eq!(execute_function(&function, &[argument]), Ok(Some(expected)));
    }
    let bytes = module(
        "(module (func (param i32) (result i32)
            local.get 0 i32.eqz i32.const 41 i32.add))",
    );
    let framed = scan_module(&bytes).unwrap();
    let composed_declarations = decode_declarations(&framed).unwrap();
    let composed_bodies = decode_bodies(&framed).unwrap();
    let composed = validate_function(&composed_declarations, 0, &composed_bodies[0]).unwrap();
    assert_eq!(execute_function(&composed, &[0]), Ok(Some(42)));
    assert_eq!(execute_function(&composed, &[1]), Ok(Some(41)));
    let instructions = expression("i32.eqz");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &body),
        Err(FunctionError::StackUnderflow {
            offset: 20,
            needed: 1,
            available: 0,
        })
    ));
}

#[test]
fn executes_nested_if_branches_and_skips_untaken_traps() {
    let wat = "(module (func (param i32 i32) (result i32)
            local.get 0
            if (result i32)
                local.get 1
                if (result i32) i32.const 11 else i32.const 22 end
                else
                    i32.const 44 i32.const 2 i32.div_s
                end))";
    let bytes = module(wat);
    let framed = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&framed).unwrap();
    let bodies = decode_bodies(&framed).unwrap();
    let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
    assert_eq!(execute_function(&function, &[0, 1]), Ok(Some(22)));
    assert_eq!(execute_function(&function, &[1, 0]), Ok(Some(22)));
    assert_eq!(execute_function(&function, &[-1, 1]), Ok(Some(11)));

    let bytes = module(
        "(module (func (param i32) (result i32)
            local.get 0 if (result i32) i32.const 42
            else i32.const 1 i32.const 0 i32.div_s end))",
    );
    let framed = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&framed).unwrap();
    let bodies = decode_bodies(&framed).unwrap();
    let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
    assert_eq!(execute_function(&function, &[1]), Ok(Some(42)));
    assert!(matches!(
        execute_function(&function, &[0]),
        Err(ExecutionError::IntegerDivideByZero { .. })
    ));

    let bytes = module(
        "(module (func (param i32) (result i32) (local i32)
            local.get 0 if i32.const 42 local.set 1 end local.get 1))",
    );
    let framed = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&framed).unwrap();
    let bodies = decode_bodies(&framed).unwrap();
    let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
    assert_eq!(execute_function(&function, &[0]), Ok(Some(0)));
    assert_eq!(execute_function(&function, &[1]), Ok(Some(42)));

    let bytes = module(
        "(module (func (param i32) (result i32)
            i32.const 10 local.get 0
            if (result i32) i32.const 2 else i32.const 3 end
            i32.add))",
    );
    let framed = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&framed).unwrap();
    let bodies = decode_bodies(&framed).unwrap();
    let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
    assert_eq!(execute_function(&function, &[0]), Ok(Some(13)));
    assert_eq!(execute_function(&function, &[1]), Ok(Some(12)));
}

#[test]
fn validates_if_branches_and_protects_the_outer_stack() {
    let declarations = Declarations {
        types: vec![FunctionType {
            parameters: vec![],
            results: vec![ValueType::I32],
        }],
        function_types: vec![],
    };
    let cases = [
        ("if end", "underflow"),
        ("i32.const 99 i32.const 1 if i32.eqz end", "underflow"),
        (
            "i32.const 1 if (result i32) i32.const 1 end",
            "missing_else",
        ),
        ("i32.const 1 if (result i32) i32.const 1 else end", "branch"),
        ("i32.const 1 if (result i32) else i32.const 1 end", "branch"),
        ("i32.const 1 if i32.const 1 end", "branch"),
    ];
    for (source, expected) in cases {
        let instructions = expression(source);
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 0,
            instructions: &instructions,
        };
        let error = match validate_function(&declarations, 0, &body) {
            Ok(_) => panic!("unexpectedly accepted {source}"),
            Err(error) => error,
        };
        assert!(
            match expected {
                "underflow" => matches!(error, FunctionError::StackUnderflow { available: 0, .. }),
                "missing_else" => matches!(error, FunctionError::MissingElseForResult { .. }),
                "branch" => matches!(error, FunctionError::BranchStackMismatch { .. }),
                _ => false,
            },
            "{source}: {error:?}"
        );
    }

    // Deeply nested valid input must not use the native call stack.
    let source = format!(
        "{}{}i32.const 42",
        "i32.const 1 if ".repeat(4096),
        "end ".repeat(4096)
    );
    let instructions = expression(&source);
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 0,
        instructions: &instructions,
    };
    let function = validate_function(&declarations, 0, &body).unwrap();
    assert_eq!(execute_function(&function, &[]), Ok(Some(42)));
}

#[test]
fn executes_blocks_loops_and_value_carrying_branches() {
    let examples: &[(&str, &[i32], i32)] = &[
        (
            "(module (func (param i32) (result i32) (local i32)
                loop local.get 1 local.get 0 i32.add local.set 1
                    local.get 0 i32.const 1 i32.sub local.tee 0 br_if 0
                end local.get 1))",
            &[5],
            15,
        ),
        (
            "(module (func (result i32) i32.const 10 block (result i32)
                i32.const 99 i32.const 32 br 0 i32.add end i32.add))",
            &[],
            42,
        ),
        (
            "(module (func (param i32) (result i32) block (result i32)
                i32.const 37 local.get 0 br_if 0 i32.const 5 i32.add end))",
            &[0],
            42,
        ),
        (
            "(module (func (param i32) (result i32) block (result i32)
                i32.const 37 local.get 0 br_if 0 i32.const 5 i32.add end))",
            &[1],
            37,
        ),
        (
            "(module (func (result i32) block (result i32) block (result i32)
                i32.const 42 br 1 end i32.const 3 i32.add end))",
            &[],
            42,
        ),
        ("(module (func (result i32) i32.const 42 br 0))", &[], 42),
        (
            "(module (func (result i32) block (result i32)
                i32.const 1 if i32.const 42 br 1 end i32.const 0 end))",
            &[],
            42,
        ),
        (
            "(module (func (result i32) loop (result i32) i32.const 42 end))",
            &[],
            42,
        ),
    ];
    for (wat, args, expected) in examples {
        let bytes = module(wat);
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        assert_eq!(
            execute_function(&function, args),
            Ok(Some(*expected)),
            "{wat}"
        );
    }
}

#[test]
fn validates_branches_and_limits_infinite_loops() {
    let declarations = Declarations {
        types: vec![FunctionType {
            parameters: vec![],
            results: vec![],
        }],
        function_types: vec![],
    };
    let instructions = expression("br 1");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &body),
        Err(FunctionError::InvalidLabelIndex {
            index: 1,
            offset: 20,
        })
    ));
    let instructions = expression("block (result i32) i32.const 1 br_if 0 i32.const 42 end");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &body),
        Err(FunctionError::StackUnderflow {
            needed: 1,
            available: 0,
            ..
        })
    ));
    let instructions = expression("block br 0 i32.const 1 end");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &body),
        Err(FunctionError::BranchStackMismatch {
            expected: 0,
            actual: 1,
            ..
        })
    ));
    let instructions = expression("loop br 0 end");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &instructions,
    };
    let function = validate_function(&declarations, 0, &body).unwrap();
    assert_eq!(
        execute_function_with_fuel(&function, &[], 20),
        Err(ExecutionError::FuelExhausted)
    );
    assert_eq!(
        execute_function_with_fuel(&function, &[], 0),
        Err(ExecutionError::FuelExhausted)
    );
}

#[test]
fn executes_i32_bitwise_shifts_and_rotations() {
    let cases = [
        ("i32.and", 0x71, 0x12345678, 0x0f0f0f0f, 0x02040608),
        ("i32.or", 0x72, i32::MIN, 1, i32::MIN + 1),
        ("i32.xor", 0x73, 5, 3, 6),
        ("i32.shl", 0x74, 1, 31, i32::MIN),
        ("i32.shl", 0x74, 1, 32, 1),
        ("i32.shl", 0x74, 1, -1, i32::MIN),
        ("i32.shr_s", 0x75, i32::MIN, 1, -1073741824),
        ("i32.shr_s", 0x75, -1, 32, -1),
        ("i32.shr_u", 0x76, -1, 1, i32::MAX),
        ("i32.shr_u", 0x76, i32::MIN, -1, 1),
        ("i32.rotl", 0x77, 1, -1, i32::MIN),
        ("i32.rotr", 0x78, 1, 33, i32::MIN),
        ("i32.rotr", 0x78, i32::MIN, -1, 1),
    ];
    for (op, opcode, left, right, expected) in cases {
        assert_eq!(expression(op), [opcode, 0x0b]);
        let bytes = module(&format!(
            "(module (func (param i32 i32) (result i32) local.get 0 local.get 1 {op}))"
        ));
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        assert_eq!(
            execute_function(&function, &[left, right]),
            Ok(Some(expected)),
            "{op} {left} {right}"
        );
        let instructions = expression(&format!("local.get 0 {op}"));
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(
            matches!(
                validate_function(&declarations, 0, &body),
                Err(FunctionError::StackUnderflow {
                    offset: 22,
                    needed: 2,
                    available: 1,
                })
            ),
            "{op}"
        );
    }
}

#[test]
fn executes_i32_bit_counts_including_zero() {
    let cases = [
        ("i32.clz", 0x67, 0, 32),
        ("i32.clz", 0x67, 1, 31),
        ("i32.clz", 0x67, -1, 0),
        ("i32.ctz", 0x68, 0, 32),
        ("i32.ctz", 0x68, i32::MIN, 31),
        ("i32.ctz", 0x68, 12, 2),
        ("i32.popcnt", 0x69, 0, 0),
        ("i32.popcnt", 0x69, -1, 32),
        ("i32.popcnt", 0x69, 0x12345678, 13),
    ];
    for (op, opcode, input, expected) in cases {
        assert_eq!(expression(op), [opcode, 0x0b]);
        let bytes = module(&format!(
            "(module (func (param i32) (result i32) local.get 0 {op}))"
        ));
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        assert_eq!(
            execute_function(&function, &[input]),
            Ok(Some(expected)),
            "{op} {input}"
        );
        let instructions = expression(op);
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(
            matches!(
                validate_function(&declarations, 0, &body),
                Err(FunctionError::StackUnderflow {
                    offset: 20,
                    needed: 1,
                    available: 0
                })
            ),
            "{op}"
        );
    }
}

#[test]
fn validates_return_and_unreachable_stack_effects() {
    let declarations = Declarations {
        types: vec![FunctionType {
            parameters: vec![],
            results: vec![ValueType::I32],
        }],
        function_types: vec![],
    };
    for source in ["return", "i32.const 42 i32.const 1 if return end"] {
        let instructions = expression(source);
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 0,
            instructions: &instructions,
        };
        assert!(
            matches!(
                validate_function(&declarations, 0, &body),
                Err(FunctionError::StackUnderflow {
                    needed: 1,
                    available: 0,
                    ..
                })
            ),
            "{source}"
        );
    }
    let instructions = expression("unreachable i32.const 1 i32.const 2");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 0,
        instructions: &instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &body),
        Err(FunctionError::ResultStackMismatch {
            expected: 1,
            actual: 2,
            ..
        })
    ));

    let instructions = expression("unreachable local.get 0");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 0,
        instructions: &instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &body),
        Err(FunctionError::InvalidLocalIndex { index: 0, .. })
    ));
}

#[test]
fn validates_drop_and_select_stack_effects() {
    let declarations = Declarations {
        types: vec![FunctionType {
            parameters: vec![],
            results: vec![ValueType::I32],
        }],
        function_types: vec![],
    };
    for (op, inputs, needed) in [
        ("drop", 0, 1),
        ("select", 0, 3),
        ("select", 1, 3),
        ("select", 2, 3),
    ] {
        let source = format!("{}{op}", "i32.const 1 ".repeat(inputs));
        let instructions = expression(&source);
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(
            matches!(
                validate_function(&declarations, 0, &body),
                Err(FunctionError::StackUnderflow {
                    offset,
                    needed: n,
                    available
                }) if offset == 20 + inputs * 2 && n == needed && available == inputs
            ),
            "{source}"
        );
    }
    let instructions = expression("i32.const 1 i32.const 1 if i32.const 2 i32.const 3 select end");
    let body = FunctionBody {
        locals: vec![],
        instructions_offset: 0,
        instructions: &instructions,
    };
    assert!(matches!(
        validate_function(&declarations, 0, &body),
        Err(FunctionError::StackUnderflow {
            needed: 3,
            available: 2,
            ..
        })
    ));

    for source in [
        "block (result i32) i32.const 42 br 0 select end",
        "block br 0 drop end i32.const 42",
    ] {
        let instructions = expression(source);
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 0,
            instructions: &instructions,
        };
        let function = validate_function(&declarations, 0, &body).unwrap();
        assert_eq!(execute_function(&function, &[]), Ok(Some(42)), "{source}");
    }
}

#[test]
fn executes_function_parameters_and_checks_local_indices() {
    let bytes = module(
        r#"(module (func (export "f") (param i32 i32) (result i32)
            local.get 0 local.get 1 i32.add))"#,
    );
    let module = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&module).unwrap();
    let bodies = decode_bodies(&module).unwrap();
    let linked = link_functions(&declarations, &bodies).unwrap();
    let (type_index, body) = linked.iter().next().unwrap();
    let function = validate_function(&declarations, type_index, body).unwrap();
    assert_eq!(execute_function(&function, &[20, 22]).unwrap(), Some(42));
    assert_eq!(
        execute_function(&function, &[i32::MAX, 1]).unwrap(),
        Some(i32::MIN)
    );
    assert_eq!(
        execute_function(&function, &[1]),
        Err(ExecutionError::ArgumentCountMismatch {
            expected: 2,
            actual: 1
        })
    );
    assert_eq!(
        execute_function(&function, &[1, 2, 3]),
        Err(ExecutionError::ArgumentCountMismatch {
            expected: 2,
            actual: 3,
        })
    );

    let bad_index_instructions = expression("local.get 2");
    let bad_index = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &bad_index_instructions,
    };
    assert!(matches!(
        validate_function(&declarations, type_index, &bad_index),
        Err(FunctionError::InvalidLocalIndex {
            index: 2,
            offset: 20,
        })
    ));
    let huge_index_instructions = expression("local.get 4294967295");
    let huge_index = FunctionBody {
        locals: vec![],
        instructions_offset: 20,
        instructions: &huge_index_instructions,
    };
    assert!(matches!(
        validate_function(&declarations, type_index, &huge_index),
        Err(FunctionError::InvalidLocalIndex {
            index: u32::MAX,
            offset: 20,
        })
    ));
}

#[test]
fn reads_zero_initialized_locals_after_parameters() {
    let bytes = module(
        r#"(module (func (export "f") (param i32) (result i32)
            (local i32 i32) local.get 0 local.get 2 i32.add))"#,
    );
    let module = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&module).unwrap();
    let bodies = decode_bodies(&module).unwrap();
    let linked = link_functions(&declarations, &bodies).unwrap();
    let (type_index, body) = linked.iter().next().unwrap();
    let function = validate_function(&declarations, type_index, body).unwrap();
    assert_eq!(execute_function(&function, &[57]).unwrap(), Some(57));
    assert_eq!(execute_function(&function, &[-5]).unwrap(), Some(-5));

    let bad_instructions = expression("local.get 3");
    let bad = FunctionBody {
        locals: vec![LocalGroup {
            count: 2,
            value_type: ValueType::I32,
        }],
        instructions_offset: 20,
        instructions: &bad_instructions,
    };
    assert!(matches!(
        validate_function(&declarations, type_index, &bad),
        Err(FunctionError::InvalidLocalIndex {
            index: 3,
            offset: 20,
        })
    ));

    let many_local_instructions = expression("local.get 1000000000");
    let many_locals = FunctionBody {
        locals: vec![LocalGroup {
            count: 1_000_000_000,
            value_type: ValueType::I32,
        }],
        instructions_offset: 20,
        instructions: &many_local_instructions,
    };
    let function = validate_function(&declarations, type_index, &many_locals).unwrap();
    assert_eq!(execute_function(&function, &[57]).unwrap(), Some(0));
}

#[test]
fn executes_local_set_and_tee_with_isolated_call_state() {
    let bytes = module(
        r#"(module (func (export "f") (param i32) (result i32) (local i32)
            local.get 0 i32.const 1 i32.add local.tee 1 local.set 0
            local.get 1 local.get 0 i32.add))"#,
    );
    let module = scan_module(&bytes).unwrap();
    let declarations = decode_declarations(&module).unwrap();
    let bodies = decode_bodies(&module).unwrap();
    let linked = link_functions(&declarations, &bodies).unwrap();
    let (index, body) = linked.iter().next().unwrap();
    let function = validate_function(&declarations, index, body).unwrap();
    assert_eq!(execute_function(&function, &[20]).unwrap(), Some(42));
    assert_eq!(execute_function(&function, &[-5]).unwrap(), Some(-8));

    let overwrite_instructions = expression("i32.const 0 local.set 0 local.get 0");
    let overwrite_with_zero = FunctionBody {
        locals: vec![],
        instructions_offset: 0,
        instructions: &overwrite_instructions,
    };
    let zero_function = validate_function(&declarations, index, &overwrite_with_zero).unwrap();
    assert_eq!(execute_function(&zero_function, &[7]).unwrap(), Some(0));

    let huge_instructions = expression("i32.const 42 local.set 1000000000 local.get 1000000000");
    let huge = FunctionBody {
        locals: vec![LocalGroup {
            count: 1_000_000_000,
            value_type: ValueType::I32,
        }],
        instructions_offset: 0,
        instructions: &huge_instructions,
    };
    let huge_function = validate_function(&declarations, index, &huge).unwrap();
    assert_eq!(execute_function(&huge_function, &[0]).unwrap(), Some(42));
}

#[test]
fn validates_local_writes_before_execution() {
    let declarations = Declarations {
        types: vec![FunctionType {
            parameters: vec![ValueType::I32],
            results: vec![ValueType::I32],
        }],
        function_types: vec![],
    };
    for opcode in ["local.set", "local.tee"] {
        let instructions = expression(&format!("{opcode} 0"));
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(matches!(
            validate_function(&declarations, 0, &body),
            Err(FunctionError::StackUnderflow {
                offset: 20,
                needed: 1,
                available: 0,
            })
        ));
        let instructions = expression(&format!("i32.const 1 {opcode} 1"));
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(matches!(
            validate_function(&declarations, 0, &body),
            Err(FunctionError::InvalidLocalIndex {
                index: 1,
                offset: 22,
            })
        ));
    }
}
