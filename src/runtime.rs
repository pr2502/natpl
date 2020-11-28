use std::collections::{HashMap, HashSet};

use crate::{
    syntax::{Expression, HasFC, InfixOp, Item, LineItem, Name, PrefixOp, SiPrefix, FC},
    term::{Unit, Value, ValueKind},
};

use bigdecimal::{BigDecimal, ToPrimitive};
use thiserror::Error;

#[derive(Default)]
pub struct Runtime {
    units: HashSet<Name>,
    variables: HashMap<Name, Value>,
    functions: HashMap<Name, (Vec<Name>, Expression)>,
}

impl Runtime {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn eval_line_item(&mut self, item: LineItem) -> Result<EvalResult, ItemError> {
        match self.line_item_to_item(item) {
            Some(item) => self.eval_item(item),
            None => Ok(EvalResult::Empty),
        }
    }

    pub fn line_item_to_item(&mut self, item: LineItem) -> Option<Item> {
        let it = match item {
            LineItem::Empty => return None,
            LineItem::UnitDeclaration(fc, name) => Item::UnitDeclaration(fc, name),
            LineItem::MaybeDeclarationOrEqualityExpression(decl) => {
                let name = decl.declaration_name();

                let is_defined = self.units.contains(name)
                    || self.variables.contains_key(name)
                    || self.functions.contains_key(name);

                if is_defined {
                    decl.into_expression()
                } else {
                    decl.into_declaration()
                }
            }
            LineItem::PrintedExpression(fc, expr) => Item::PrintedExpression(fc, expr),
            LineItem::SilentExpression(expr) => Item::SilentExpression(expr),
        };
        Some(it)
    }

    fn eval_item(&mut self, item: Item) -> Result<EvalResult, ItemError> {
        use std::collections::hash_map::Entry;

        match item {
            Item::UnitDeclaration(_, name) => {
                let name = name.name();

                let is_unique = self.units.insert(name.clone());
                if !is_unique {
                    Err(ItemError::UnitRedeclared(name))
                } else {
                    Ok(EvalResult::Empty)
                }
            }
            Item::VariableDeclaration { fc: _, name, rhs } => {
                let value = self.eval_expr(&rhs)?;

                let name = name.name();
                match self.variables.entry(name.clone()) {
                    Entry::Occupied(_) => Err(ItemError::VariableRedefined(name)),
                    Entry::Vacant(entry) => {
                        let val = entry.insert(value).clone();
                        Ok(EvalResult::Value(val))
                    }
                }
            }
            Item::FunctionDeclaration {
                fc: _,
                name,
                arg_names,
                rhs,
            } => {
                let name = name.name();

                match self.functions.entry(name.clone()) {
                    Entry::Occupied(_) => Err(ItemError::FunctionRedefined(name)),
                    Entry::Vacant(entry) => {
                        entry.insert((arg_names.into_iter().map(|n| n.name()).collect(), rhs));
                        Ok(EvalResult::Empty)
                    }
                }
            }
            Item::PrintedExpression(_, e) => {
                let val = self.eval_expr(&e)?;
                Ok(EvalResult::PrintValue(e, val))
            }
            Item::SilentExpression(e) => {
                let val = self.eval_expr(&e)?;
                Ok(EvalResult::Value(val))
            }
        }
    }

    fn eval_expr(&self, expr: &Expression) -> Result<Value, EvalError> {
        match expr {
            Expression::IntegerLit { fc: _, val } => Ok(Value {
                kind: ValueKind::Number(val.clone()),
                unit: Unit::new(),
            }),
            Expression::FloatLit { fc: _, val } => Ok(Value {
                kind: ValueKind::Number(val.clone()),
                unit: Unit::new(),
            }),
            Expression::MaybeUnitPrefix {
                fc,
                name,
                full_name,
                prefix,
            } => {
                if let Some(val) = self.lookup(full_name) {
                    return Ok(val);
                }

                if let Some(val) = self.lookup(name) {
                    apply_prefix(*fc, *prefix, val)
                } else {
                    Err(EvalError::UndefinedName(*fc, full_name.clone()))
                }
            }
            Expression::Variable(id) => self
                .lookup(id.name_ref())
                .ok_or_else(|| EvalError::UndefinedName(id.fc(), id.name_ref().clone())),
            Expression::Call {
                fc: _,
                base: _,
                args: _,
            } => {
                todo!()
            }
            Expression::PrefixOp { fc, op, expr } => {
                let mut val = self.eval_expr(expr)?;
                match op {
                    crate::syntax::PrefixOp::Pos => match &mut val.kind {
                        ValueKind::Number(_) => Ok(val),
                        ValueKind::FunctionRef(_) => {
                            Err(EvalError::InvalidPrefixOperator(*fc, *op, val))
                        }
                    },
                    crate::syntax::PrefixOp::Neg => match &mut val.kind {
                        ValueKind::Number(num) => {
                            *num = -&*num;
                            Ok(val)
                        }
                        ValueKind::FunctionRef(_) => {
                            Err(EvalError::InvalidPrefixOperator(*fc, *op, val))
                        }
                    },
                }
            }
            Expression::InfixOp { fc, op, lhs, rhs } => self.eval_infix_op(*fc, *op, lhs, rhs),
            Expression::UnitOf(_, expr) => {
                let val = self.eval_expr(expr)?;
                Ok(Value {
                    kind: ValueKind::Number(BigDecimal::from(1)),
                    unit: val.unit,
                })
            }
            Expression::Parenthesised(_, expr) => self.eval_expr(expr),
        }
    }

    pub fn lookup(&self, name: &Name) -> Option<Value> {
        if let Some(val) = self.variables.get(name) {
            Some(val.clone())
        } else if self.units.contains(name) {
            Some(Value {
                kind: ValueKind::Number(BigDecimal::from(1)),
                unit: Unit::new_named(name.clone()),
            })
        } else if self.functions.contains_key(name) {
            Some(Value {
                kind: ValueKind::FunctionRef(name.clone()),
                unit: Unit::new(),
            })
        } else {
            None
        }
    }

    fn eval_infix_op(
        &self,
        fc: FC,
        op: InfixOp,
        lhs: &Expression,
        rhs: &Expression,
    ) -> Result<Value, EvalError> {
        let lhs = self.eval_expr(lhs)?;
        let rhs = self.eval_expr(rhs)?;

        let unit = infix_unit(fc, op, &lhs, &rhs)?;

        match (op, &lhs.kind, &rhs.kind) {
            (InfixOp::Add, ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value {
                kind: ValueKind::Number(a + b),
                unit,
            }),
            (InfixOp::Sub, ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value {
                kind: ValueKind::Number(a - b),
                unit,
            }),
            (InfixOp::Mul, ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value {
                kind: ValueKind::Number(a * b),
                unit,
            }),
            (InfixOp::Div, ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value {
                kind: ValueKind::Number(a / b),
                unit,
            }),
            (InfixOp::Mod, ValueKind::Number(a), ValueKind::Number(b)) => Ok(Value {
                kind: ValueKind::Number(a % b),
                unit,
            }),
            (InfixOp::Pow, ValueKind::Number(a), ValueKind::Number(b)) => {
                let pow: isize = if b.is_integer() {
                    b.to_isize().unwrap()
                } else {
                    unimplemented!("Floating point power is not implemented")
                };

                let mut res = BigDecimal::from(1);

                for _ in 0..pow.abs() {
                    res = res * a;
                }

                if pow.is_negative() {
                    res = res.inverse();
                }

                Ok(Value {
                    kind: ValueKind::Number(res),
                    unit,
                })
            }
            (InfixOp::Eq, ValueKind::Number(_), ValueKind::Number(_)) => {
                todo!()
            }
            (InfixOp::Neq, ValueKind::Number(_), ValueKind::Number(_)) => {
                todo!()
            }
            (InfixOp::Gt, ValueKind::Number(_), ValueKind::Number(_)) => {
                todo!()
            }
            (op, _, _) => Err(EvalError::InvalidInfixOperator(fc, op, lhs, rhs)),
        }
    }
}

fn apply_prefix(fc: FC, prefix: SiPrefix, mut val: Value) -> Result<Value, EvalError> {
    let kind = match (prefix, &val.kind) {
        (SiPrefix::Femto, ValueKind::Number(x)) => ValueKind::Number(x / 1_000_000_000_000_000u64),
        (SiPrefix::Pico, ValueKind::Number(x)) => ValueKind::Number(x / 1_000_000_000_000u64),
        (SiPrefix::Nano, ValueKind::Number(x)) => ValueKind::Number(x / 1_000_000_000u64),
        (SiPrefix::Micro, ValueKind::Number(x)) => ValueKind::Number(x / 1_000_000u64),
        (SiPrefix::Milli, ValueKind::Number(x)) => ValueKind::Number(x / 1_000u64),
        (SiPrefix::Centi, ValueKind::Number(x)) => ValueKind::Number(x / 100u64),
        (SiPrefix::Deci, ValueKind::Number(x)) => ValueKind::Number(x / 10u64),
        (SiPrefix::Deca, ValueKind::Number(x)) => ValueKind::Number(x * BigDecimal::from(10u64)),
        (SiPrefix::Hecto, ValueKind::Number(x)) => ValueKind::Number(x * BigDecimal::from(100u64)),
        (SiPrefix::Kilo, ValueKind::Number(x)) => ValueKind::Number(x * BigDecimal::from(1_000u64)),
        (SiPrefix::Mega, ValueKind::Number(x)) => {
            ValueKind::Number(x * BigDecimal::from(1_000_000u64))
        }
        (SiPrefix::Giga, ValueKind::Number(x)) => {
            ValueKind::Number(x * BigDecimal::from(1_000_000_000u64))
        }
        (SiPrefix::Tera, ValueKind::Number(x)) => {
            ValueKind::Number(x * BigDecimal::from(1_000_000_000_000u64))
        }
        (SiPrefix::Peta, ValueKind::Number(x)) => {
            ValueKind::Number(x * BigDecimal::from(1_000_000_000_000_000u64))
        }
        (_, ValueKind::FunctionRef(_)) => return Err(EvalError::InvalidSiPrefix(fc, prefix, val)),
    };
    val.kind = kind;
    Ok(val)
}

fn infix_unit(fc: FC, op: InfixOp, lhs: &Value, rhs: &Value) -> Result<Unit, UnitError> {
    match op {
        InfixOp::Add | InfixOp::Sub | InfixOp::Mod => {
            if lhs.unit == rhs.unit {
                Ok(lhs.unit.clone())
            } else {
                Err(UnitError::IncompatibleUnits(
                    fc,
                    op,
                    lhs.unit.clone(),
                    rhs.unit.clone(),
                ))
            }
        }

        InfixOp::Mul => Ok(lhs.unit.multiply(&rhs.unit)),
        InfixOp::Div => Ok(lhs.unit.divide(&rhs.unit)),
        InfixOp::Pow => {
            if rhs.unit != Unit::new() {
                return Err(UnitError::IncompatibleUnits(
                    fc,
                    op,
                    lhs.unit.clone(),
                    rhs.unit.clone(),
                ));
            }
            match &rhs.kind {
                ValueKind::Number(n) if n.is_integer() => {
                    let n = n.to_isize().unwrap();
                    Ok(lhs.unit.pow(n))
                }
                _ => Err(UnitError::InvalidPowerValue(
                    fc,
                    lhs.unit.clone(),
                    rhs.kind.clone(),
                )),
            }
        }
        InfixOp::Eq => todo!(),
        InfixOp::Neq => todo!(),
        InfixOp::Gt => todo!(),
    }
}

pub enum EvalResult {
    Empty,
    Value(Value),
    PrintValue(Expression, Value),
}

#[derive(Debug, Error)]
pub enum ItemError {
    #[error("Unit redeclared: {}", .0)]
    UnitRedeclared(Name),
    #[error("Variable redefined: {}", .0)]
    VariableRedefined(Name),
    #[error("Function redefined: {}", .0)]
    FunctionRedefined(Name),

    #[error("Eval error: {}", .0)]
    EvalError(#[from] EvalError),
}

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("Undefined name: {}", .1)]
    UndefinedName(FC, Name),

    #[error("Invalid prefix operator {:?} on value {:?}", .1, .2)]
    InvalidPrefixOperator(FC, PrefixOp, Value),

    #[error("Invalid infix operator {:?} on {:?} and {:?}", .1, .2, .3)]
    InvalidInfixOperator(FC, InfixOp, Value, Value),

    #[error("Invalid SI-prefix {:?} on value {:?}", .1, .2)]
    InvalidSiPrefix(FC, SiPrefix, Value),

    #[error("Unit error: {}", .0)]
    UnitError(#[from] UnitError),
}

#[derive(Debug, Error)]
pub enum UnitError {
    #[error("Incompatible units ({}) and ({}) for operation {:?}", .2, .3, .1)]
    IncompatibleUnits(FC, InfixOp, Unit, Unit),

    #[error("Invalid power on unit ({}): {}", .1, .2)]
    InvalidPowerValue(FC, Unit, ValueKind),
}