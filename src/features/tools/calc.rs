//! `calculate` tool (spec §9.3): evaluating a math expression.
//!
//! Its own recursive-descent evaluator (no external crates): arithmetic
//! `+ - * / %`, exponentiation `^` (right-associative), unary minus, parens,
//! constants (`pi`, `e`, `tau`), and functions (`sqrt`, `sin`, `log`, `min`, `max`, …).
//! The pure function [`eval`] is fully testable. The tool needs **no** I/O and isn't
//! gated by any switches (safe).

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::shared::i18n::Locale;

use super::{Tool, ToolContext, ToolOutcome};

/// `calculate` — evaluates an arithmetic/math expression.
pub struct Calculate;

#[async_trait::async_trait]
impl Tool for Calculate {
    fn id(&self) -> ToolId {
        "calculate".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Utils
    }
    fn ui_label(&self) -> &'static str {
        "calculator"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.calculate.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": loc.t("tool.calculate.param.expression")
                }
            },
            "required": ["expression"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let expr = args
            .get("expression")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.calculate.err.expr_empty")))?;

        match eval(expr, ctx.loc) {
            Ok(value) => Ok(ToolOutcome::text(format!(
                "{expr} = {}",
                format_number(value, ctx.loc)
            ))),
            Err(err) => Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.calculate.result.failed",
                &[("expr", expr), ("err", &err.to_string())],
            ))),
        }
    }
}

/// Formats the result: integers — with no fractional part, otherwise trailing zeros are trimmed.
fn format_number(v: f64, loc: &crate::shared::i18n::Locale) -> String {
    if v.is_nan() {
        return loc.t("calc.number.nan").to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "∞" } else { "-∞" }.to_string();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    // Up to 12 significant digits, no trailing zeros.
    let s = format!("{v:.12}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

// ------------------------- Lexer -------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Comma,
}

/// Splits a string into tokens. Errors on an unknown character. `loc` — the error language.
fn tokenize(input: &str, loc: &Locale) -> Result<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            c if c.is_ascii_digit() || c == '.' => {
                let end = number_end(&chars, i);
                let s: String = chars[i..end].iter().collect();
                let n = s
                    .parse::<f64>()
                    .map_err(|_| anyhow::anyhow!(loc.tf("calc.err.bad_number", &[("s", &s)])))?;
                tokens.push(Token::Number(n));
                i = end;
            }
            c if c.is_alphabetic() || c == '_' => {
                let end = scan_while(&chars, i, |c| c.is_alphanumeric() || c == '_');
                let s: String = chars[i..end].iter().collect();
                tokens.push(Token::Ident(s.to_lowercase()));
                i = end;
            }
            other => {
                let token = single_char_token(other).ok_or_else(|| {
                    anyhow::anyhow!(loc.tf("calc.err.unknown_char", &[("c", &other.to_string())]))
                })?;
                tokens.push(token);
                i += 1;
            }
        }
    }
    Ok(tokens)
}

/// The token a one-character operator or delimiter stands for; `None` for any other character.
fn single_char_token(c: char) -> Option<Token> {
    Some(match c {
        '+' => Token::Plus,
        '-' => Token::Minus,
        '*' => Token::Star,
        '/' => Token::Slash,
        '%' => Token::Percent,
        '^' => Token::Caret,
        '(' => Token::LParen,
        ')' => Token::RParen,
        ',' => Token::Comma,
        _ => return None,
    })
}

/// The index of the first character at or after `from` that `keep` rejects (or the end).
fn scan_while(chars: &[char], from: usize, keep: impl Fn(char) -> bool) -> usize {
    chars[from..]
        .iter()
        .position(|&c| !keep(c))
        .map_or(chars.len(), |n| from + n)
}

/// Where a number starting at `start` ends: digits and dots, then an optional exponent
/// (`1e3`, `2.5E-4`). The span is taken greedily and parsed afterwards, so a malformed
/// one (`1.2.3`, `2e`) is reported whole as a bad number rather than split into tokens.
fn number_end(chars: &[char], start: usize) -> usize {
    let mut i = scan_while(chars, start, |c| c.is_ascii_digit() || c == '.');
    if matches!(chars.get(i), Some('e' | 'E')) {
        i += 1;
        if matches!(chars.get(i), Some('+' | '-')) {
            i += 1;
        }
        i = scan_while(chars, i, |c| c.is_ascii_digit());
    }
    i
}

// ------------------------- Parser/evaluator -------------------------

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    loc: &'a Locale,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// expr := term (('+' | '-') term)*
    fn expr(&mut self) -> Result<f64> {
        let mut value = self.term()?;
        while let Some(op) = self.peek() {
            match op {
                Token::Plus => {
                    self.next();
                    value += self.term()?;
                }
                Token::Minus => {
                    self.next();
                    value -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    /// term := factor (('*' | '/' | '%') factor)*
    fn term(&mut self) -> Result<f64> {
        let mut value = self.factor()?;
        while let Some(op) = self.peek() {
            match op {
                Token::Star => {
                    self.next();
                    value *= self.factor()?;
                }
                Token::Slash => {
                    self.next();
                    let rhs = self.factor()?;
                    value /= rhs;
                }
                Token::Percent => {
                    self.next();
                    let rhs = self.factor()?;
                    value %= rhs;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    /// factor := unary ('^' factor)?   (exponentiation is right-associative)
    fn factor(&mut self) -> Result<f64> {
        let base = self.unary()?;
        if let Some(Token::Caret) = self.peek() {
            self.next();
            let exp = self.factor()?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    /// unary := ('-' | '+') unary | atom
    fn unary(&mut self) -> Result<f64> {
        match self.peek() {
            Some(Token::Minus) => {
                self.next();
                Ok(-self.unary()?)
            }
            Some(Token::Plus) => {
                self.next();
                self.unary()
            }
            _ => self.atom(),
        }
    }

    /// atom := number | const | func '(' args ')' | '(' expr ')'
    fn atom(&mut self) -> Result<f64> {
        match self.next() {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::LParen) => {
                let v = self.expr()?;
                self.expect(Token::RParen)?;
                Ok(v)
            }
            Some(Token::Ident(name)) => {
                // A function if the identifier is followed by "(".
                if let Some(Token::LParen) = self.peek() {
                    self.next();
                    let mut args = vec![self.expr()?];
                    while let Some(Token::Comma) = self.peek() {
                        self.next();
                        args.push(self.expr()?);
                    }
                    self.expect(Token::RParen)?;
                    apply_function(&name, &args, self.loc)
                } else {
                    constant(&name, self.loc)
                }
            }
            other => {
                anyhow::bail!(self.loc.tf(
                    "calc.err.unexpected_token",
                    &[("token", &format!("{other:?}"))]
                ))
            }
        }
    }

    fn expect(&mut self, want: Token) -> Result<()> {
        match self.next() {
            Some(t) if t == want => Ok(()),
            other => anyhow::bail!(self.loc.tf(
                "calc.err.expected",
                &[
                    ("want", &format!("{want:?}")),
                    ("found", &format!("{other:?}"))
                ]
            )),
        }
    }
}

/// The value of a named constant.
fn constant(name: &str, loc: &Locale) -> Result<f64> {
    match name {
        "pi" => Ok(std::f64::consts::PI),
        "e" => Ok(std::f64::consts::E),
        "tau" => Ok(std::f64::consts::TAU),
        other => anyhow::bail!(loc.tf("calc.err.unknown_const", &[("name", other)])),
    }
}

/// Applies a function to its arguments (checking arity). `loc` — the error language.
fn apply_function(name: &str, args: &[f64], loc: &Locale) -> Result<f64> {
    let one = |a: &[f64]| -> Result<f64> {
        if a.len() != 1 {
            anyhow::bail!(loc.tf(
                "calc.err.arity_one",
                &[("name", name), ("n", &a.len().to_string())]
            ));
        }
        Ok(a[0])
    };
    let two = |a: &[f64]| -> Result<(f64, f64)> {
        if a.len() != 2 {
            anyhow::bail!(loc.tf(
                "calc.err.arity_two",
                &[("name", name), ("n", &a.len().to_string())]
            ));
        }
        Ok((a[0], a[1]))
    };
    Ok(match name {
        "sqrt" => one(args)?.sqrt(),
        "cbrt" => one(args)?.cbrt(),
        "abs" => one(args)?.abs(),
        "exp" => one(args)?.exp(),
        "ln" => one(args)?.ln(),
        "log2" => one(args)?.log2(),
        // log(x) — base 10; log(x, base) — to a given base.
        "log" | "log10" => {
            if args.len() == 2 {
                args[0].log(args[1])
            } else {
                one(args)?.log10()
            }
        }
        "sin" => one(args)?.sin(),
        "cos" => one(args)?.cos(),
        "tan" => one(args)?.tan(),
        "asin" => one(args)?.asin(),
        "acos" => one(args)?.acos(),
        "atan" => one(args)?.atan(),
        "atan2" => {
            let (y, x) = two(args)?;
            y.atan2(x)
        }
        "sinh" => one(args)?.sinh(),
        "cosh" => one(args)?.cosh(),
        "tanh" => one(args)?.tanh(),
        "floor" => one(args)?.floor(),
        "ceil" => one(args)?.ceil(),
        "round" => one(args)?.round(),
        "pow" => {
            let (b, e) = two(args)?;
            b.powf(e)
        }
        "min" => {
            if args.is_empty() {
                anyhow::bail!(loc.tf("calc.err.needs_args", &[("name", "min")]));
            }
            args.iter().copied().fold(f64::INFINITY, f64::min)
        }
        "max" => {
            if args.is_empty() {
                anyhow::bail!(loc.tf("calc.err.needs_args", &[("name", "max")]));
            }
            args.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        }
        other => anyhow::bail!(loc.tf("calc.err.unknown_func", &[("name", other)])),
    })
}

/// Evaluates the expression as `f64`. Error texts — in the scaffold language `loc`.
pub fn eval(input: &str, loc: &Locale) -> Result<f64> {
    let tokens = tokenize(input, loc)?;
    if tokens.is_empty() {
        anyhow::bail!(loc.t("calc.err.empty").to_string());
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        loc,
    };
    let value = parser.expr()?;
    if parser.pos != parser.tokens.len() {
        anyhow::bail!(loc.t("calc.err.extra_tokens").to_string());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    /// Reference locale (ru) for pure evaluations in tests.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }
    /// Thin wrappers: tests call `eval`/`format_number` with no explicit locale (shadowing
    /// the module's same-named functions via `super::`). Pin the ru bundle.
    fn eval(s: &str) -> Result<f64> {
        super::eval(s, ru())
    }
    fn format_number(v: f64) -> String {
        super::format_number(v, ru())
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert!(approx(eval("2 + 3 * 4").unwrap(), 14.0));
        assert!(approx(eval("(2 + 3) * 4").unwrap(), 20.0));
        assert!(approx(eval("10 / 4").unwrap(), 2.5));
        assert!(approx(eval("10 % 3").unwrap(), 1.0));
        assert!(approx(eval("-5 + 2").unwrap(), -3.0));
        assert!(approx(eval("2 - -3").unwrap(), 5.0));
    }

    #[test]
    fn power_is_right_associative() {
        // 2^3^2 = 2^(3^2) = 2^9 = 512, not (2^3)^2 = 64.
        assert!(approx(eval("2^3^2").unwrap(), 512.0));
        assert!(approx(eval("2^10").unwrap(), 1024.0));
    }

    #[test]
    fn functions_and_constants() {
        assert!(approx(eval("sqrt(16)").unwrap(), 4.0));
        assert!(approx(eval("sin(pi/2)").unwrap(), 1.0));
        assert!(approx(eval("max(3, 7, 5)").unwrap(), 7.0));
        assert!(approx(eval("min(3, 7, 5)").unwrap(), 3.0));
        assert!(approx(eval("log(1000)").unwrap(), 3.0));
        assert!(approx(eval("log(8, 2)").unwrap(), 3.0));
        assert!(approx(eval("pow(2, 10)").unwrap(), 1024.0));
        assert!(approx(eval("abs(-4.5)").unwrap(), 4.5));
    }

    #[test]
    fn scientific_notation() {
        assert!(approx(eval("1e3").unwrap(), 1000.0));
        assert!(approx(eval("2.5e-2").unwrap(), 0.025));
    }

    #[test]
    fn the_tokenizer_takes_numbers_and_names_whole() {
        let tokens = |s: &str| tokenize(s, ru()).unwrap();
        assert_eq!(
            tokens("2.5E-4*Sin(x_1)"),
            vec![
                Token::Number(2.5e-4),
                Token::Star,
                Token::Ident("sin".into()),
                Token::LParen,
                Token::Ident("x_1".into()),
                Token::RParen,
            ]
        );
        assert_eq!(
            tokens(" 1e+3^2 % 4, -.5 / 7"),
            vec![
                Token::Number(1000.0),
                Token::Caret,
                Token::Number(2.0),
                Token::Percent,
                Token::Number(4.0),
                Token::Comma,
                Token::Minus,
                Token::Number(0.5),
                Token::Slash,
                Token::Number(7.0),
            ]
        );
        // A malformed number is reported as the whole span, not cut where it stops parsing.
        for bad in ["1.2.3", "2e", "3E+x"] {
            let err = tokenize(bad, ru()).unwrap_err().to_string();
            let span = bad.trim_end_matches('x');
            assert!(err.contains(&format!("«{span}»")), "{bad:?}: {err}");
        }
        assert!(tokenize("2 @", ru()).is_err());
    }

    #[test]
    fn errors_on_bad_input() {
        assert!(eval("").is_err());
        assert!(eval("2 +").is_err());
        assert!(eval("2 ) (").is_err());
        assert!(eval("foobar(2)").is_err());
        assert!(eval("nope").is_err());
        assert!(eval("sqrt(1, 2)").is_err()); // wrong arity
        assert!(eval("@").is_err()); // unknown character
        assert!(eval("2 3").is_err()); // extra tokens
    }

    #[test]
    fn format_number_trims() {
        assert_eq!(format_number(4.0), "4");
        assert_eq!(format_number(2.5), "2.5");
        assert_eq!(format_number(-3.0), "-3");
    }

    #[tokio::test]
    async fn tool_returns_result_string() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = Calculate
            .invoke(&ctx, serde_json::json!({"expression": "(2 + 3) * 4"}))
            .await
            .unwrap();
        assert!(out.result.contains("20"), "got: {}", out.result);
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn tool_reports_error_not_panic() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = Calculate
            .invoke(&ctx, serde_json::json!({"expression": "2 +"}))
            .await
            .unwrap();
        assert!(
            out.result.contains("Не удалось вычислить"),
            "got: {}",
            out.result
        );
    }

    #[tokio::test]
    async fn tool_rejects_empty_expression() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        assert!(
            Calculate
                .invoke(&ctx, serde_json::json!({"expression": "  "}))
                .await
                .is_err()
        );
    }
}
