//! System tasks at evaluation time: turn a `va_ir::Stmt::Task`'s arguments into the text and
//! the `va_abi::TaskKind` that `va_abi::StampSink::task` carries (LRM §9.4, §9.7).
//!
//! Formatting follows LRM §9.4.3: a string argument is printed literally except that `%`
//! starts a format specification consuming the next argument; an expression argument no
//! specification consumed is printed in the default decimal format; an empty argument (`,,`)
//! prints one space. Supported specifications: `%d %h %x %o %b %c %s %e %f %g %r %m %l %%`, in
//! either case, with the C-style `-`/`0` flags, a width and a precision.
//!
//! **Stated limitations.** `%l` prints `work.<module>` (this engine has no library binding to
//! report); `%s` of a numeric argument prints its bytes as ASCII, most significant first, as the
//! LRM describes; a specification with no argument left to consume is printed literally rather
//! than refused, since a message is the wrong place to fail a simulation.

use crate::ad::{Ctx, Dual};
use crate::{tape, CodegenError};
use va_abi::{FileOp, TaskKind};
use va_ir::{TaskArg, TaskKind as IrKind, VarId};

/// A multichannel descriptor's channels are bits 1..=30 (bit 0 is standard output, bit 31 is
/// reserved), so one instance can hold at most this many files open through `$fopen(name)`.
const MCD_CHANNELS: usize = 30;

/// The standard file descriptors (LRM §9.5.1): `32'h8000_0001` and `32'h8000_0002`.
const FD_STDOUT: u64 = 0x8000_0001;
const FD_STDERR: u64 = 0x8000_0002;
/// The first file descriptor `$fopen(name, type)` hands out; `+ k` for table entry `k`.
const FD_FIRST: u64 = 0x8000_0003;

/// Where a descriptor's output goes.
enum Dest {
    Stdout,
    Stderr,
    File(String),
}

/// Run one task statement: evaluate and format its arguments, and queue what it asks for on
/// `ctx` — a task line (`Ctx::tasks`), a file operation (`Ctx::file_ops`) — or, for `$fopen`,
/// assign the descriptor to `target`. `monitor` is a `$monitor`'s first state slot.
pub(crate) fn run(
    ctx: &Ctx,
    kind: IrKind,
    args: &[TaskArg],
    target: Option<VarId>,
    monitor: Option<usize>,
) -> Result<(), CodegenError> {
    let instance = ctx.instance_name;
    let module = ctx.module.name.as_str();
    // Every expression argument is evaluated, in order, whether or not it is printed: an
    // argument is an expression like any other, and evaluating it is what validation checks.
    let mut vals: Vec<Val> = Vec::with_capacity(args.len());
    for a in args {
        vals.push(match a {
            TaskArg::Str(s) => Val::Str(s.clone()),
            TaskArg::Expr(e) => Val::Num(tape::eval_value(ctx, *e)?),
            TaskArg::Null => Val::Null,
        });
    }
    let level = |v: Option<&Val>| match v {
        Some(Val::Num(n)) if n.is_finite() => n.round().clamp(0.0, 2.0) as u8,
        _ => 1,
    };
    let text = |from: usize| format_args(vals.get(from..).unwrap_or(&[]), instance, module);
    let task = |k: TaskKind, t: String| ctx.tasks.borrow_mut().push((k, t));
    match kind {
        IrKind::Strobe => task(TaskKind::Strobe, text(0)),
        IrKind::Display => task(TaskKind::Display, text(0)),
        IrKind::Write => task(TaskKind::Write, text(0)),
        IrKind::Debug => task(TaskKind::Debug, text(0)),
        IrKind::Error => task(TaskKind::Error, text(0)),
        IrKind::Warning => task(TaskKind::Warning, text(0)),
        IrKind::Info => task(TaskKind::Info, text(0)),
        // Their text is where they were called from (`%m`): the location LRM Table 9-25's
        // level 1 reports.
        IrKind::Finish => task(TaskKind::Finish(level(vals.first())), instance.to_string()),
        IrKind::Stop => task(TaskKind::Stop(level(vals.first())), instance.to_string()),
        // `$fatal(finish_number, message...)`: the number, when given, is not part of the text.
        IrKind::Fatal => match vals.first() {
            Some(Val::Num(_)) => task(TaskKind::Fatal(level(vals.first())), text(1)),
            _ => task(TaskKind::Fatal(1), text(0)),
        },
        IrKind::Monitor => {
            if monitor_changed(ctx, kind, args, &vals, monitor) {
                task(TaskKind::Strobe, text(0));
            }
        }
        IrKind::Fopen => fopen(ctx, &vals, target),
        IrKind::Fclose | IrKind::Fflush if vals.is_empty() => {
            // `$fflush` with no descriptor: every open file (LRM §9.5.6).
            ctx.file_ops
                .borrow_mut()
                .push((FileOp::Flush, None, String::new()));
        }
        IrKind::Fclose | IrKind::Fflush => {
            let op = if kind == IrKind::Fclose {
                FileOp::Close
            } else {
                FileOp::Flush
            };
            for dest in destinations(ctx, vals.first()) {
                if let Dest::File(path) = dest {
                    ctx.file_ops
                        .borrow_mut()
                        .push((op, Some(path), String::new()));
                }
            }
        }
        IrKind::Fdisplay | IrKind::Fstrobe => write(ctx, &vals, FileOp::Write { newline: true }),
        IrKind::Fwrite => write(ctx, &vals, FileOp::Write { newline: false }),
        IrKind::Fdebug => write(ctx, &vals, FileOp::Debug),
        IrKind::Fmonitor => {
            if monitor_changed(ctx, kind, args, &vals, monitor) {
                write(ctx, &vals, FileOp::Write { newline: true });
            }
        }
    }
    Ok(())
}

/// `var = $fopen(name[, type])`: the descriptor of `name` in this instance's file table (added
/// if new) — a multichannel descriptor without a type, a file descriptor with one — assigned to
/// `target`, and an `Open` queued. Opening the same name twice hands back the same channel. A
/// multichannel table that is full returns 0, the LRM's "could not be opened", with a warning.
fn fopen(ctx: &Ctx, vals: &[Val], target: Option<VarId>) {
    let Some(Val::Str(path)) = vals.first() else {
        return;
    };
    let mode = match vals.get(1) {
        Some(Val::Str(m)) => Some(m.as_str()),
        _ => None,
    };
    let k = {
        let mut table = ctx
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match table.iter().position(|p| p == path) {
            Some(k) => k,
            None => {
                table.push(path.clone());
                table.len() - 1
            }
        }
    };
    let descriptor = match mode {
        Some(_) => (FD_FIRST + k as u64) as f64,
        None if k < MCD_CHANNELS => (1u64 << (k + 1)) as f64,
        None => {
            ctx.tasks.borrow_mut().push((
                TaskKind::Warning,
                format!(
                    "{}: $fopen(\"{path}\"): no multichannel descriptor left (at most {MCD_CHANNELS} files); it returns 0",
                    ctx.instance_name
                ),
            ));
            0.0
        }
    };
    if let Some(v) = target {
        ctx.set_var(v, Dual::constant(descriptor));
    }
    if descriptor != 0.0 {
        let append = mode.is_some_and(|m| m.starts_with('a'));
        ctx.file_ops.borrow_mut().push((
            FileOp::Open { append },
            Some(path.clone()),
            String::new(),
        ));
    }
}

/// Where descriptor `d` writes (LRM §9.5.1): a file descriptor (bit 31 set) names one stream; a
/// multichannel descriptor names every channel whose bit is set, bit 0 being standard output. A
/// channel this instance never opened is reported once per evaluation as a warning and skipped.
fn destinations(ctx: &Ctx, d: Option<&Val>) -> Vec<Dest> {
    let Some(Val::Num(v)) = d else {
        return Vec::new();
    };
    if !v.is_finite() {
        return Vec::new();
    }
    // Verilog's integers are 32 bits: a descriptor held in a signed variable reads negative.
    let u = (v.round() as i64 as u64) & 0xFFFF_FFFF;
    let table = ctx
        .files
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = Vec::new();
    let mut unknown = false;
    if u & 0x8000_0000 != 0 {
        match u {
            FD_STDOUT => out.push(Dest::Stdout),
            FD_STDERR => out.push(Dest::Stderr),
            _ => match u.checked_sub(FD_FIRST).and_then(|k| table.get(k as usize)) {
                Some(p) => out.push(Dest::File(p.clone())),
                None => unknown = true,
            },
        }
    } else {
        if u & 1 != 0 {
            out.push(Dest::Stdout);
        }
        for bit in 1..=MCD_CHANNELS {
            if u & (1 << bit) != 0 {
                match table.get(bit - 1) {
                    Some(p) => out.push(Dest::File(p.clone())),
                    None => unknown = true,
                }
            }
        }
    }
    drop(table);
    if unknown {
        ctx.tasks.borrow_mut().push((
            TaskKind::Warning,
            format!(
                "{}: a file task wrote to descriptor {u:#x}, which no $fopen of this instance returned; that output is lost",
                ctx.instance_name
            ),
        ));
    }
    out
}

/// A file-output task: its text (every argument after the descriptor) to each of the
/// descriptor's destinations — standard output as the matching display task, standard error and
/// files as file operations.
fn write(ctx: &Ctx, vals: &[Val], op: FileOp) {
    let text = format_args(
        vals.get(1..).unwrap_or(&[]),
        ctx.instance_name,
        &ctx.module.name,
    );
    for dest in destinations(ctx, vals.first()) {
        match dest {
            Dest::Stdout => {
                let kind = match op {
                    FileOp::Write { newline: false } => TaskKind::Write,
                    FileOp::Debug => TaskKind::Debug,
                    _ => TaskKind::Display,
                };
                ctx.tasks.borrow_mut().push((kind, text.clone()));
            }
            Dest::Stderr => ctx.file_ops.borrow_mut().push((op, None, text.clone())),
            Dest::File(path) => ctx
                .file_ops
                .borrow_mut()
                .push((op, Some(path), text.clone())),
        }
    }
}

/// `$monitor`'s test (LRM §9.4.1): whether this is its first accepted step, or any compared
/// argument ([`crate::lower::monitor_compared`]) differs from what it saw at the last accepted
/// one, read from the committed state. Writes this evaluation's values to the proposal; the
/// consumer commits it only if the step is accepted, which is what makes the comparison "with
/// the last accepted step". Without state slots (a static analysis, or a `$monitor` in a
/// function body) it always reports a change.
fn monitor_changed(
    ctx: &Ctx,
    kind: IrKind,
    args: &[TaskArg],
    vals: &[Val],
    monitor: Option<usize>,
) -> bool {
    let Some(base) = monitor else {
        return true;
    };
    let compared: Vec<f64> = crate::lower::monitor_compared(ctx.module, kind, args)
        .into_iter()
        .map(|i| match vals.get(i) {
            Some(Val::Num(v)) => *v,
            _ => 0.0,
        })
        .collect();
    let prev = |i: usize| ctx.state_prev.get(base + i).copied();
    let changed = prev(0) != Some(1.0)
        || compared
            .iter()
            .enumerate()
            .any(|(i, v)| prev(1 + i).map(f64::to_bits) != Some(v.to_bits()));
    let mut next = ctx.state_next.borrow_mut();
    if let Some(flag) = next.get_mut(base) {
        *flag = 1.0;
    }
    for (i, v) in compared.iter().enumerate() {
        if let Some(slot) = next.get_mut(base + 1 + i) {
            *slot = *v;
        }
    }
    changed
}

/// One evaluated argument.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    Str(String),
    Num(f64),
    Null,
}

/// The text a display task prints for `vals` (no trailing newline: the consumer adds it for
/// every task but `$write`).
pub(crate) fn format_args(vals: &[Val], instance: &str, module: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < vals.len() {
        match &vals[i] {
            Val::Str(fmt) => {
                i += 1;
                format_string(fmt, vals, &mut i, instance, module, &mut out);
            }
            Val::Num(v) => {
                out.push_str(&default_number(*v));
                i += 1;
            }
            Val::Null => {
                out.push(' ');
                i += 1;
            }
        }
    }
    out
}

/// Expand one format string, consuming arguments from `vals[*next..]`.
fn format_string(
    fmt: &str,
    vals: &[Val],
    next: &mut usize,
    instance: &str,
    module: &str,
    out: &mut String,
) {
    let chars: Vec<char> = fmt.chars().collect();
    let mut k = 0;
    while k < chars.len() {
        let c = chars[k];
        if c != '%' {
            out.push(c);
            k += 1;
            continue;
        }
        // `%[-][0][width][.prec]conv`
        let start = k;
        k += 1;
        let mut left = false;
        let mut zero = false;
        while k < chars.len() && (chars[k] == '-' || chars[k] == '0') {
            if chars[k] == '-' {
                left = true;
            } else {
                zero = true;
            }
            k += 1;
        }
        let mut width = 0usize;
        while k < chars.len() && chars[k].is_ascii_digit() {
            width = width * 10 + chars[k].to_digit(10).unwrap_or(0) as usize;
            k += 1;
        }
        let mut prec: Option<usize> = None;
        if k < chars.len() && chars[k] == '.' {
            k += 1;
            let mut p = 0usize;
            while k < chars.len() && chars[k].is_ascii_digit() {
                p = p * 10 + chars[k].to_digit(10).unwrap_or(0) as usize;
                k += 1;
            }
            prec = Some(p);
        }
        let Some(&conv) = chars.get(k) else {
            out.extend(&chars[start..]);
            return;
        };
        k += 1;
        let literal = || chars[start..k].iter().collect::<String>();
        let body = match conv.to_ascii_lowercase() {
            '%' => "%".to_string(),
            'm' => instance.to_string(),
            'l' => format!("work.{module}"),
            conv => {
                let Some(arg) = vals.get(*next) else {
                    out.push_str(&literal());
                    continue;
                };
                *next += 1;
                match (conv, arg) {
                    ('s', Val::Str(s)) => s.clone(),
                    ('s', Val::Num(v)) => ascii_of(*v),
                    (_, Val::Null) => " ".to_string(),
                    (_, Val::Str(s)) => s.clone(),
                    ('d', Val::Num(v)) => int_of(*v).to_string(),
                    ('h' | 'x', Val::Num(v)) => format!("{:x}", bits_of(*v)),
                    ('o', Val::Num(v)) => format!("{:o}", bits_of(*v)),
                    ('b', Val::Num(v)) => format!("{:b}", bits_of(*v)),
                    ('c', Val::Num(v)) => char::from_u32(int_of(*v) as u32)
                        .map(String::from)
                        .unwrap_or_default(),
                    ('e', Val::Num(v)) => c_exp(*v, prec.unwrap_or(6), conv.is_uppercase()),
                    ('f', Val::Num(v)) => c_fixed(*v, prec.unwrap_or(6)),
                    ('g', Val::Num(v)) => c_general(*v, prec.unwrap_or(6), conv.is_uppercase()),
                    ('r', Val::Num(v)) => engineering(*v, prec.unwrap_or(6)),
                    _ => {
                        // An unknown specification: printed as written, its argument unused.
                        *next -= 1;
                        literal()
                    }
                }
            }
        };
        pad(&body, width, left, zero, out);
    }
}

fn pad(body: &str, width: usize, left: bool, zero: bool, out: &mut String) {
    let n = body.chars().count();
    if n >= width {
        out.push_str(body);
    } else if left {
        out.push_str(body);
        out.extend(std::iter::repeat_n(' ', width - n));
    } else if zero && body.starts_with('-') {
        out.push('-');
        out.extend(std::iter::repeat_n('0', width - n));
        out.push_str(&body[1..]);
    } else {
        let fill = if zero { '0' } else { ' ' };
        out.extend(std::iter::repeat_n(fill, width - n));
        out.push_str(body);
    }
}

/// Verilog's real-to-integer conversion: round half away from zero.
fn int_of(v: f64) -> i64 {
    if v.is_finite() {
        v.round() as i64
    } else {
        0
    }
}

/// The bits `%h`/`%o`/`%b` print: the integer, two's complement in 32 bits when negative.
fn bits_of(v: f64) -> u64 {
    let i = int_of(v);
    if i < 0 {
        u64::from(i as u32)
    } else {
        i as u64
    }
}

/// `%s` of a number: its bytes as characters, most significant first, leading zeros skipped.
fn ascii_of(v: f64) -> String {
    let mut n = int_of(v) as u64;
    let mut bytes = Vec::new();
    while n > 0 {
        bytes.push((n & 0xff) as u8);
        n >>= 8;
    }
    bytes.iter().rev().map(|&b| b as char).collect()
}

/// The default decimal format for an expression no specification consumed: an integral value
/// as an integer, anything else as `%g`.
fn default_number(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        c_general(v, 6, false)
    }
}

fn non_finite(v: f64) -> Option<String> {
    if v.is_nan() {
        Some("nan".into())
    } else if v.is_infinite() {
        Some(if v > 0.0 { "inf" } else { "-inf" }.into())
    } else {
        None
    }
}

/// C's `%.<prec>e`: one digit, `prec` decimals, an exponent of at least two digits.
fn c_exp(v: f64, prec: usize, upper: bool) -> String {
    if let Some(s) = non_finite(v) {
        return s;
    }
    let s = format!("{v:.prec$e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    let sign = if e < 0 { '-' } else { '+' };
    let out = format!("{mant}e{sign}{:02}", e.abs());
    if upper {
        out.to_uppercase()
    } else {
        out
    }
}

/// C's `%.<prec>f`.
fn c_fixed(v: f64, prec: usize) -> String {
    non_finite(v).unwrap_or_else(|| format!("{v:.prec$}"))
}

/// C's `%.<prec>g`: `%e` or `%f` by the exponent, trailing zeros removed.
fn c_general(v: f64, prec: usize, upper: bool) -> String {
    if let Some(s) = non_finite(v) {
        return s;
    }
    let p = prec.max(1);
    if v == 0.0 {
        return "0".into();
    }
    // The exponent after rounding to `p` significant digits, which is what C decides on.
    let rounded = format!("{v:.prec$e}", prec = p - 1);
    let exp: i32 = rounded
        .split_once('e')
        .and_then(|(_, e)| e.parse().ok())
        .unwrap_or(0);
    if exp < -4 || exp >= p as i32 {
        let s = c_exp(v, p - 1, upper);
        let (mant, e) = s.split_once(['e', 'E']).unwrap_or((&s, ""));
        let mant = strip_zeros(mant);
        let marker = if upper { 'E' } else { 'e' };
        format!("{mant}{marker}{e}")
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        strip_zeros(&format!("{v:.decimals$}")).to_string()
    }
}

fn strip_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// `%r`: engineering notation with the LRM §2.6.2 scale factors, `%e` outside their range.
fn engineering(v: f64, prec: usize) -> String {
    if let Some(s) = non_finite(v) {
        return s;
    }
    if v == 0.0 {
        return "0".into();
    }
    const SCALES: [(i32, &str); 9] = [
        (12, "T"),
        (9, "G"),
        (6, "M"),
        (3, "K"),
        (0, ""),
        (-3, "m"),
        (-6, "u"),
        (-9, "n"),
        (-12, "p"),
    ];
    let exp3 = (libm::log10(v.abs()).floor() as i32).div_euclid(3) * 3;
    let extra = [(-15, "f"), (-18, "a")];
    let found = SCALES
        .iter()
        .chain(extra.iter())
        .find(|(e, _)| *e == exp3)
        .copied();
    match found {
        Some((e, suffix)) => {
            let m = v / 10f64.powi(e);
            format!("{}{suffix}", c_general(m, prec, false))
        }
        None => c_exp(v, prec, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(vals: Vec<Val>) -> String {
        format_args(&vals, "X1", "diode")
    }
    fn s(t: &str) -> Val {
        Val::Str(t.into())
    }

    /// The specifications of LRM Tables 9-22/9-23 against what C's printf (and so every SPICE
    /// that prints a `$strobe`) gives for the same value.
    #[test]
    fn format_specifications_match_c_and_the_lrm() {
        let n = Val::Num;
        assert_eq!(fmt(vec![s("v=%g"), n(0.5)]), "v=0.5");
        assert_eq!(fmt(vec![s("%g"), n(1.0e-7)]), "1e-07");
        assert_eq!(fmt(vec![s("%g"), n(123456789.0)]), "1.23457e+08");
        assert_eq!(fmt(vec![s("%e"), n(1234.5)]), "1.234500e+03");
        assert_eq!(fmt(vec![s("%.3e"), n(-0.000123)]), "-1.230e-04");
        assert_eq!(fmt(vec![s("%f"), n(2.5)]), "2.500000");
        assert_eq!(fmt(vec![s("%10.3f|"), n(1.23456)]), "     1.235|");
        assert_eq!(fmt(vec![s("%-6d|"), n(42.0)]), "42    |");
        assert_eq!(fmt(vec![s("%05d"), n(-42.0)]), "-0042");
        assert_eq!(fmt(vec![s("%d"), n(2.6)]), "3");
        assert_eq!(
            fmt(vec![s("%h %o %b"), n(255.0), n(8.0), n(5.0)]),
            "ff 10 101"
        );
        assert_eq!(fmt(vec![s("%c"), n(65.0)]), "A");
        assert_eq!(
            fmt(vec![s("%s and %s"), s("text"), n(16706.0)]),
            "text and AB"
        );
        assert_eq!(fmt(vec![s("%r"), n(4.7e-9)]), "4.7n");
        assert_eq!(fmt(vec![s("%r"), n(2200.0)]), "2.2K");
        assert_eq!(fmt(vec![s("%m: %l 100%%")]), "X1: work.diode 100%");
    }

    /// The parts of §9.4.1 that are not format specifications: a leftover expression prints in
    /// the default format, an empty argument prints a space, a later string is a new format.
    #[test]
    fn arguments_without_a_specification() {
        let n = Val::Num;
        assert_eq!(fmt(vec![s("a="), n(3.0), Val::Null, n(0.25)]), "a=3 0.25");
        assert_eq!(
            fmt(vec![s("%d "), n(1.0), s("then %d"), n(2.0)]),
            "1 then 2"
        );
        assert_eq!(fmt(vec![]), "");
        // Too few arguments: the specification is printed as written, nothing fails.
        assert_eq!(fmt(vec![s("x=%g y=%g"), n(1.0)]), "x=1 y=%g");
    }
}
