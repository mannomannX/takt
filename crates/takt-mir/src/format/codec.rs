//! Codec: `Field` fuer jeden Wert, der als Feld eines Knotens stehen kann,
//! und die Makros, die Struct- und Enum-Codecs aus einer Feldliste mit
//! Nummern erzeugen (`schema.rs`).

use takt_diag::{FileId, Span};

use super::wire::{FormatError, Node, Raw, Reader, Result, Wire, Writer, unzigzag};

/// Ein Wert als Feld eines Knotens.
pub trait Field: Sized {
    /// Drahtart.
    const WIRE: Wire;
    /// Name fuer Fehlermeldungen.
    const NAME: &'static str;
    /// Schreibt das Feld `tag`.
    fn write(&self, w: &mut Writer, tag: u32);
    /// Liest einen Wert.
    fn read(raw: Raw<'_>, r: &Reader) -> Result<Self>;
}

/// Primitive kennen ihre Feldnummer nicht; sie melden 0, der Aufrufer setzt sie ein.
fn at(tag: u32) -> impl Fn(FormatError) -> FormatError {
    move |e| match e {
        FormatError::WrongWire(n, 0) => FormatError::WrongWire(n, tag),
        FormatError::OutOfRange(n, 0) => FormatError::OutOfRange(n, tag),
        other => other,
    }
}

/// Genau ein Wert.
pub fn read_one<T: Field>(n: &Node<'_>, r: &Reader, tag: u32) -> Result<T> {
    T::read(n.one(tag)?, r).map_err(at(tag))
}

/// Hoechstens ein Wert.
pub fn read_opt<T: Field>(n: &Node<'_>, r: &Reader, tag: u32) -> Result<Option<T>> {
    n.opt(tag)?.map(|raw| T::read(raw, r).map_err(at(tag))).transpose()
}

/// Alle Werte.
pub fn read_rep<T: Field>(n: &Node<'_>, r: &Reader, tag: u32) -> Result<Vec<T>> {
    n.all(tag).map(|raw| T::read(raw, r).map_err(at(tag))).collect()
}

/// Metadatum: fehlt in der Logikform, dann Default.
pub fn read_meta<T: Field + Default>(n: &Node<'_>, r: &Reader, tag: u32) -> Result<T> {
    Ok(read_opt(n, r, tag)?.unwrap_or_default())
}

/// Schreibt einen optionalen Wert.
pub fn write_opt<T: Field>(v: &Option<T>, w: &mut Writer, tag: u32) {
    if let Some(v) = v {
        v.write(w, tag);
    }
}

/// Schreibt alle Werte.
pub fn write_rep<T: Field>(v: &[T], w: &mut Writer, tag: u32) {
    for x in v {
        x.write(w, tag);
    }
}

/// Varint eines Felds.
pub fn varint(raw: Raw<'_>, name: &'static str) -> Result<u64> {
    match raw {
        Raw::Varint(v) => Ok(v),
        _ => Err(FormatError::WrongWire(name, 0)),
    }
}

fn bytes<'a>(raw: Raw<'a>, name: &'static str) -> Result<&'a [u8]> {
    match raw {
        Raw::Bytes(b) => Ok(b),
        _ => Err(FormatError::WrongWire(name, 0)),
    }
}

macro_rules! unsigned {
    ($($t:ty),*) => {$(
        impl Field for $t {
            const WIRE: Wire = Wire::Varint;
            const NAME: &'static str = stringify!($t);
            fn write(&self, w: &mut Writer, tag: u32) {
                w.varint(tag, u64::from(*self));
            }
            fn read(raw: Raw<'_>, _: &Reader) -> Result<Self> {
                <$t>::try_from(varint(raw, Self::NAME)?).map_err(|_| FormatError::OutOfRange(Self::NAME, 0))
            }
        }
    )*};
}
unsigned!(u8, u16, u32, u64);

macro_rules! signed {
    ($($t:ty),*) => {$(
        impl Field for $t {
            const WIRE: Wire = Wire::Varint;
            const NAME: &'static str = stringify!($t);
            fn write(&self, w: &mut Writer, tag: u32) {
                w.signed(tag, i64::from(*self));
            }
            fn read(raw: Raw<'_>, _: &Reader) -> Result<Self> {
                <$t>::try_from(unzigzag(varint(raw, Self::NAME)?)).map_err(|_| FormatError::OutOfRange(Self::NAME, 0))
            }
        }
    )*};
}
signed!(i8, i64);

impl Field for bool {
    const WIRE: Wire = Wire::Varint;
    const NAME: &'static str = "bool";
    fn write(&self, w: &mut Writer, tag: u32) {
        w.varint(tag, u64::from(*self));
    }
    fn read(raw: Raw<'_>, _: &Reader) -> Result<Self> {
        match varint(raw, Self::NAME)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(FormatError::OutOfRange(Self::NAME, 0)),
        }
    }
}

impl Field for f64 {
    const WIRE: Wire = Wire::Fixed64;
    const NAME: &'static str = "f64";
    fn write(&self, w: &mut Writer, tag: u32) {
        w.fixed64(tag, self.to_bits());
    }
    fn read(raw: Raw<'_>, _: &Reader) -> Result<Self> {
        match raw {
            Raw::Fixed64(v) => Ok(f64::from_bits(v)),
            _ => Err(FormatError::WrongWire(Self::NAME, 0)),
        }
    }
}

impl Field for String {
    const WIRE: Wire = Wire::Varint;
    const NAME: &'static str = "String";
    fn write(&self, w: &mut Writer, tag: u32) {
        w.string(tag, self);
    }
    fn read(raw: Raw<'_>, r: &Reader) -> Result<Self> {
        Ok(r.string(varint(raw, Self::NAME)?)?.to_string())
    }
}

impl<T: Field> Field for Box<T> {
    const WIRE: Wire = T::WIRE;
    const NAME: &'static str = T::NAME;
    fn write(&self, w: &mut Writer, tag: u32) {
        (**self).write(w, tag);
    }
    fn read(raw: Raw<'_>, r: &Reader) -> Result<Self> {
        T::read(raw, r).map(Box::new)
    }
}

/// Paare als Knoten mit den Feldern 1 und 2.
impl<A: Field, B: Field> Field for (A, B) {
    const WIRE: Wire = Wire::Bytes;
    const NAME: &'static str = "Paar";
    fn write(&self, w: &mut Writer, tag: u32) {
        w.begin();
        self.0.write(w, 1);
        self.1.write(w, 2);
        w.end(tag);
    }
    fn read(raw: Raw<'_>, r: &Reader) -> Result<Self> {
        let n = Node::parse(Self::NAME, bytes(raw, Self::NAME)?)?;
        Ok((read_one(&n, r, 1)?, read_one(&n, r, 2)?))
    }
}

/// Dimensionsvektor als Bytefolge (7 Bytes, Zweierkomplement).
impl Field for [i8; crate::types::BASE_DIMENSIONS] {
    const WIRE: Wire = Wire::Bytes;
    const NAME: &'static str = "Dimension";
    fn write(&self, w: &mut Writer, tag: u32) {
        let b: Vec<u8> = self.iter().map(|&x| x as u8).collect();
        w.bytes(tag, &b);
    }
    fn read(raw: Raw<'_>, _: &Reader) -> Result<Self> {
        let b = bytes(raw, Self::NAME)?;
        let arr: [u8; crate::types::BASE_DIMENSIONS] =
            b.try_into().map_err(|_| FormatError::OutOfRange(Self::NAME, 0))?;
        Ok(arr.map(|x| x as i8))
    }
}

/// Positionen: Datei, Anfang, Ende.
impl Field for Span {
    const WIRE: Wire = Wire::Bytes;
    const NAME: &'static str = "Span";
    fn write(&self, w: &mut Writer, tag: u32) {
        w.begin();
        w.varint(1, u64::from(self.file.0));
        w.varint(2, u64::from(self.start));
        w.varint(3, u64::from(self.end));
        w.end(tag);
    }
    fn read(raw: Raw<'_>, r: &Reader) -> Result<Self> {
        let n = Node::parse(Self::NAME, bytes(raw, Self::NAME)?)?;
        Ok(Span { file: FileId(read_one(&n, r, 1)?), start: read_one(&n, r, 2)?, end: read_one(&n, r, 3)? })
    }
}

/// Liest einen Knoten aus einem Bytefeld.
pub fn node<'a>(raw: Raw<'a>, name: &'static str) -> Result<Node<'a>> {
    Node::parse(name, bytes(raw, name)?)
}

/// Codec fuer einen Index-Typ (`TypeId` usw.): Varint.
macro_rules! codec_id {
    ($($t:ident),* $(,)?) => {$(
        impl Field for $crate::ids::$t {
            const WIRE: Wire = Wire::Varint;
            const NAME: &'static str = stringify!($t);
            fn write(&self, w: &mut Writer, tag: u32) {
                w.varint(tag, u64::from(self.0));
            }
            fn read(raw: Raw<'_>, r: &Reader) -> Result<Self> {
                Ok($crate::ids::$t(u32::read(raw, r)?))
            }
        }
    )*};
}
pub(crate) use codec_id;

/// Schreibt ein Feld nach Modus: `one`, `opt`, `rep`, `meta` (Positionen,
/// Bindungen, Metadaten: nur ausserhalb der Logikform), `meta opt`.
macro_rules! write_field {
    ($w:ident, $tag:literal, one, $e:expr) => {
        Field::write(&$e, $w, $tag)
    };
    ($w:ident, $tag:literal, opt, $e:expr) => {
        write_opt(&$e, $w, $tag)
    };
    ($w:ident, $tag:literal, rep, $e:expr) => {
        write_rep(&$e, $w, $tag)
    };
    ($w:ident, $tag:literal, meta, $e:expr) => {
        if !$w.logic_only {
            Field::write(&$e, $w, $tag)
        }
    };
    ($w:ident, $tag:literal, metaopt, $e:expr) => {
        if !$w.logic_only {
            write_opt(&$e, $w, $tag)
        }
    };
}
pub(crate) use write_field;

/// Liest ein Feld nach Modus.
macro_rules! read_field {
    ($n:ident, $r:ident, $tag:literal, one) => {
        read_one(&$n, $r, $tag)?
    };
    ($n:ident, $r:ident, $tag:literal, opt) => {
        read_opt(&$n, $r, $tag)?
    };
    ($n:ident, $r:ident, $tag:literal, rep) => {
        read_rep(&$n, $r, $tag)?
    };
    ($n:ident, $r:ident, $tag:literal, meta) => {
        read_meta(&$n, $r, $tag)?
    };
    ($n:ident, $r:ident, $tag:literal, metaopt) => {
        read_opt(&$n, $r, $tag)?
    };
}
pub(crate) use read_field;

/// Struct-Codec: `codec_struct!(Name { 1 one a, 2 opt b, 3 rep c, 4 meta span })`.
macro_rules! codec_struct {
    ($name:ident { $($tag:literal $mode:ident $field:ident),* $(,)? }) => {
        impl Field for $name {
            const WIRE: Wire = Wire::Bytes;
            const NAME: &'static str = stringify!($name);
            fn write(&self, w: &mut Writer, tag: u32) {
                w.begin();
                $( write_field!(w, $tag, $mode, self.$field); )*
                w.end(tag);
            }
            fn read(raw: Raw<'_>, r: &Reader) -> Result<Self> {
                let n = node(raw, Self::NAME)?;
                Ok($name { $( $field: read_field!(n, r, $tag, $mode), )* })
            }
        }
    };
}
pub(crate) use codec_struct;

/// Codec eines Enums ohne Nutzlast: Varint der Variantennummer.
macro_rules! codec_unit_enum {
    ($name:ident { $($tag:literal $variant:ident),* $(,)? }) => {
        impl Field for $name {
            const WIRE: Wire = Wire::Varint;
            const NAME: &'static str = stringify!($name);
            fn write(&self, w: &mut Writer, tag: u32) {
                let v: u64 = match self { $( $name::$variant => $tag, )* };
                w.varint(tag, v);
            }
            fn read(raw: Raw<'_>, _: &Reader) -> Result<Self> {
                match varint(raw, Self::NAME)? {
                    $( $tag => Ok($name::$variant), )*
                    v => Err(FormatError::BadVariant(Self::NAME, v)),
                }
            }
        }
    };
}
pub(crate) use codec_unit_enum;

/// Codec eines Enums mit Nutzlast: Knoten mit Feld 0 = Variantennummer und
/// den Feldern der Variante. Schreibweise je Variante:
/// `1 Unit`, `2 Tuple(1 one a, 2 opt b)`, `3 Struct { 1 one a, 2 rep b }`.
macro_rules! codec_enum {
    ($name:ident { $($tag:literal $variant:ident $( ( $($ttag:literal $tmode:ident $tfield:ident),* ) )? $( { $($stag:literal $smode:ident $sfield:ident),* } )? ),* $(,)? }) => {
        impl Field for $name {
            const WIRE: Wire = Wire::Bytes;
            const NAME: &'static str = stringify!($name);
            #[allow(unused_variables)]
            fn write(&self, w: &mut Writer, tag: u32) {
                w.begin();
                match self {
                    $(
                        $name::$variant $( ( $($tfield),* ) )? $( { $($sfield),* } )? => {
                            w.varint(0, $tag);
                            $( $( write_field!(w, $ttag, $tmode, *$tfield); )* )?
                            $( $( write_field!(w, $stag, $smode, *$sfield); )* )?
                        }
                    )*
                }
                w.end(tag);
            }
            #[allow(unused_variables)]
            fn read(raw: Raw<'_>, r: &Reader) -> Result<Self> {
                let n = node(raw, Self::NAME)?;
                match read_one::<u64>(&n, r, 0)? {
                    $(
                        $tag => Ok($name::$variant
                            $( ( $( read_field!(n, r, $ttag, $tmode) ),* ) )?
                            $( { $( $sfield: read_field!(n, r, $stag, $smode) ),* } )?
                        ),
                    )*
                    v => Err(FormatError::BadVariant(Self::NAME, v)),
                }
            }
        }
    };
}
pub(crate) use codec_enum;
