//! Typausdruecke: `type`, `scalar_type`, `int_type`, `elem_type`.

use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind;

impl<'t, 's> Parser<'t, 's> {
    /// `type`
    pub(super) fn parse_type(&mut self) -> PResult<Type> {
        let start = self.pos;
        let kind = if self.at_op("[") {
            self.bump();
            let len = self.parse_const_expr()?;
            self.expect_op("]")?;
            let elem = self.parse_type()?;
            TypeKind::Array { len: Box::new(len), elem: Box::new(elem) }
        } else if self.is_scalar_word() {
            let scalar = self.parse_scalar_type()?;
            let range = if self.eat_kw("in") { Some(self.parse_range()?) } else { None };
            let wrap = self.parse_wrap()?;
            TypeKind::Scalar { scalar, range, wrap }
        } else if self.at(TokenKind::TypeIdent) {
            let name = self.type_ident()?;
            let wrap = self.parse_wrap()?;
            TypeKind::Named { name, wrap }
        } else if self.at(TokenKind::UpperIdent) {
            let name = self.upper()?;
            let wrap = self.parse_wrap()?;
            TypeKind::TypeVar { name, wrap }
        } else if Self::is_word(self.kind()) {
            match self.text() {
                "bytes" => {
                    self.bump();
                    TypeKind::Bytes(Box::new(self.parse_angle_expr()?))
                }
                "line" => {
                    self.bump();
                    TypeKind::Line(Box::new(self.parse_angle_expr()?))
                }
                "vec" => {
                    self.bump();
                    if self.eat_op("[") {
                        let tuple = self.parse_unit_tuple()?;
                        self.expect_op("]")?;
                        TypeKind::VecDim(tuple)
                    } else {
                        let (elem, len) = self.in_angles(|p| {
                            let elem = p.parse_type()?;
                            p.expect_op(",")?;
                            Ok((elem, p.parse_const_expr()?))
                        })?;
                        TypeKind::Vec { elem: Box::new(elem), len: Box::new(len) }
                    }
                }
                "stream" => {
                    self.bump();
                    let elem = self.in_angles(Self::parse_elem_type)?;
                    TypeKind::Stream(Box::new(elem))
                }
                "samples" => {
                    self.bump();
                    let (elem, len) = self.in_angles(|p| {
                        let elem = p.parse_type()?;
                        p.expect_op(",")?;
                        Ok((elem, p.parse_const_expr()?))
                    })?;
                    TypeKind::Samples { elem: Box::new(elem), len: Box::new(len) }
                }
                "table" => {
                    self.bump();
                    let (key, value) = self.in_angles(|p| {
                        let key = p.parse_type()?;
                        p.expect_op(",")?;
                        Ok((key, p.parse_type()?))
                    })?;
                    TypeKind::Table { key: Box::new(key), value: Box::new(value) }
                }
                "mat" => {
                    self.bump();
                    if self.eat_op("[") {
                        let rows = self.parse_unit_tuple()?;
                        self.expect_op(",")?;
                        let cols = self.parse_unit_tuple()?;
                        self.expect_op("]")?;
                        TypeKind::MatDim { rows, cols }
                    } else {
                        let (rows, cols) = self.in_angles(|p| {
                            let rows = p.parse_const_expr()?;
                            p.expect_op(",")?;
                            Ok((rows, p.parse_const_expr()?))
                        })?;
                        let unit = if self.eat_op("[") {
                            let u = self.parse_unit_expr(false)?;
                            self.expect_op("]")?;
                            Some(u)
                        } else {
                            None
                        };
                        TypeKind::Mat { rows: Box::new(rows), cols: Box::new(cols), unit }
                    }
                }
                "map" => {
                    self.bump();
                    let (key, value, len) = self.in_angles(|p| {
                        let key = p.parse_type()?;
                        p.expect_op(",")?;
                        let value = p.parse_type()?;
                        p.expect_op(",")?;
                        Ok((key, value, p.parse_const_expr()?))
                    })?;
                    TypeKind::Map { key: Box::new(key), value: Box::new(value), len: Box::new(len) }
                }
                _ => return Err(self.error_here("einen Typ")),
            }
        } else {
            return Err(self.error_here("einen Typ"));
        };
        Ok(Type { kind, span: self.span_from(start) })
    }

    /// `"<" const_expr ">"`
    fn parse_angle_expr(&mut self) -> PResult<Expr> {
        self.in_angles(Self::parse_const_expr)
    }

    /// `[ "?" | "!" TYPE_IDENT ]`
    fn parse_wrap(&mut self) -> PResult<Option<Wrap>> {
        if self.eat_op("?") {
            Ok(Some(Wrap::Optional))
        } else if self.at_op("!") {
            self.bump();
            Ok(Some(Wrap::Result(self.type_ident()?)))
        } else {
            Ok(None)
        }
    }

    /// `scalar_type`
    pub(super) fn parse_scalar_type(&mut self) -> PResult<ScalarType> {
        if !Self::is_word(self.kind()) {
            return Err(self.error_here("einen Skalartyp"));
        }
        let word = self.text();
        let scalar = match word {
            "bool" => {
                self.bump();
                ScalarType::Bool
            }
            "float" | "f32" | "f64" => {
                let width = match word {
                    "f32" => Some(FloatWidth::F32),
                    "f64" => Some(FloatWidth::F64),
                    _ => None,
                };
                self.bump();
                let unit = self.parse_bracket_unit()?;
                ScalarType::Float { width, unit }
            }
            "Duration" => {
                self.bump();
                ScalarType::Duration
            }
            "str" => {
                self.bump();
                ScalarType::Str(Box::new(self.parse_angle_expr()?))
            }
            _ => {
                let ty = self.parse_int_type()?;
                let unit = self.parse_bracket_unit()?;
                ScalarType::Int { ty, unit }
            }
        };
        Ok(scalar)
    }

    /// `[ "[" unit_expr "]" ]` hinter einem Zahlentyp; nur, wenn die Klammer anliegt.
    fn parse_bracket_unit(&mut self) -> PResult<Option<UnitExpr>> {
        let prev_joint = self.toks.tokens[self.pos - 1].joint;
        if prev_joint && self.eat_op("[") {
            let unit = self.parse_unit_expr(false)?;
            self.expect_op("]")?;
            Ok(Some(unit))
        } else {
            Ok(None)
        }
    }

    /// `int_type`
    pub(super) fn parse_int_type(&mut self) -> PResult<IntType> {
        let ty = if Self::is_word(self.kind()) {
            match self.text() {
                "int" => IntType::Int,
                "i8" => IntType::I8,
                "i16" => IntType::I16,
                "i32" => IntType::I32,
                "i64" => IntType::I64,
                "u8" => IntType::U8,
                "u16" => IntType::U16,
                "u32" => IntType::U32,
                "u64" => IntType::U64,
                _ => return Err(self.error_here("einen Integer-Typ wie `int`, `u8`, `i32`")),
            }
        } else {
            return Err(self.error_here("einen Integer-Typ wie `int`, `u8`, `i32`"));
        };
        self.bump();
        Ok(ty)
    }

    /// `elem_type`
    pub(super) fn parse_elem_type(&mut self) -> PResult<ElemType> {
        if self.at(TokenKind::TypeIdent) && self.text() != "Edge" {
            return Ok(ElemType::Named(self.type_ident()?));
        }
        if !Self::is_word(self.kind()) {
            return Err(self.error_here("einen Elementtyp wie `u8`, `bytes<N>`, `line<N>`, `Edge` oder einen Record"));
        }
        match self.text() {
            "u8" => {
                self.bump();
                Ok(ElemType::U8)
            }
            "Edge" => {
                self.bump();
                Ok(ElemType::Edge)
            }
            "bytes" => {
                self.bump();
                Ok(ElemType::Bytes(self.parse_angle_expr()?))
            }
            "line" => {
                self.bump();
                Ok(ElemType::Line(self.parse_angle_expr()?))
            }
            "capture" => {
                self.bump();
                let (elem, len) = self.in_angles(|p| {
                    let elem = p.parse_type()?;
                    p.expect_op(",")?;
                    Ok((elem, p.parse_const_expr()?))
                })?;
                Ok(ElemType::Capture { elem: Box::new(elem), len })
            }
            _ => Err(self.error_here("einen Elementtyp wie `u8`, `bytes<N>`, `line<N>`, `Edge` oder einen Record")),
        }
    }
}
