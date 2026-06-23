//! Инструмент `calculate` (spec §9.3): вычисление математического выражения.
//!
//! Собственный рекурсивно-нисходящий вычислитель (без внешних крейтов): арифметика
//! `+ - * / %`, степень `^` (правоассоциативная), унарный минус, скобки,
//! константы (`pi`, `e`, `tau`) и функции (`sqrt`, `sin`, `log`, `min`, `max`, …).
//! Чистая функция [`eval`] полностью тестируема. Инструмент **не** требует I/O и не
//! гейтится выключателями (безопасен).

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// `calculate` — вычисляет арифметическое/математическое выражение.
pub struct Calculate;

#[async_trait::async_trait]
impl Tool for Calculate {
    fn id(&self) -> ToolId {
        "calculate".into()
    }
    fn description(&self) -> String {
        "Вычислить математическое выражение и вернуть число. Поддерживает + - * / % ^, \
         скобки, константы (pi, e, tau) и функции (sqrt, cbrt, abs, exp, ln, log, log2, \
         sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, floor, ceil, round, \
         min, max, pow). Углы тригонометрии — в радианах."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Выражение, например: (2 + 3) * sqrt(16) или sin(pi/2)"
                }
            },
            "required": ["expression"]
        })
    }
    async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let expr = args
            .get("expression")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("ожидается непустое поле expression"))?;

        match eval(expr) {
            Ok(value) => Ok(ToolOutcome::text(format!(
                "{expr} = {}",
                format_number(value)
            ))),
            Err(err) => Ok(ToolOutcome::text(format!(
                "Не удалось вычислить «{expr}»: {err}"
            ))),
        }
    }
}

/// Форматирует результат: целые — без дробной части, иначе с обрезкой хвостовых нулей.
fn format_number(v: f64) -> String {
    if v.is_nan() {
        return "не число (NaN)".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "∞" } else { "-∞" }.to_string();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    // До 12 значащих знаков, без хвостовых нулей.
    let s = format!("{v:.12}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

// ------------------------- Лексер -------------------------

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

/// Разбивает строку на токены. Ошибка на неизвестном символе.
fn tokenize(input: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            '^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                // Экспоненциальная запись: 1e3, 2.5E-4.
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    i += 1;
                    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let s: String = chars[start..i].iter().collect();
                let n = s
                    .parse::<f64>()
                    .map_err(|_| anyhow::anyhow!("неверное число «{s}»"))?;
                tokens.push(Token::Number(n));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(s.to_lowercase()));
            }
            other => anyhow::bail!("неизвестный символ «{other}»"),
        }
    }
    Ok(tokens)
}

// ------------------------- Парсер/вычислитель -------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
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

    /// factor := unary ('^' factor)?   (степень правоассоциативна)
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
                // Функция, если за идентификатором идёт «(».
                if let Some(Token::LParen) = self.peek() {
                    self.next();
                    let mut args = vec![self.expr()?];
                    while let Some(Token::Comma) = self.peek() {
                        self.next();
                        args.push(self.expr()?);
                    }
                    self.expect(Token::RParen)?;
                    apply_function(&name, &args)
                } else {
                    constant(&name)
                }
            }
            other => anyhow::bail!("неожиданный токен: {other:?}"),
        }
    }

    fn expect(&mut self, want: Token) -> Result<()> {
        match self.next() {
            Some(t) if t == want => Ok(()),
            other => anyhow::bail!("ожидалось {want:?}, найдено {other:?}"),
        }
    }
}

/// Значение именованной константы.
fn constant(name: &str) -> Result<f64> {
    match name {
        "pi" => Ok(std::f64::consts::PI),
        "e" => Ok(std::f64::consts::E),
        "tau" => Ok(std::f64::consts::TAU),
        other => anyhow::bail!("неизвестная константа/имя «{other}»"),
    }
}

/// Применяет функцию к аргументам (проверяя арность).
fn apply_function(name: &str, args: &[f64]) -> Result<f64> {
    let one = |a: &[f64]| -> Result<f64> {
        if a.len() != 1 {
            anyhow::bail!("функция «{name}» ожидает 1 аргумент, дано {}", a.len());
        }
        Ok(a[0])
    };
    let two = |a: &[f64]| -> Result<(f64, f64)> {
        if a.len() != 2 {
            anyhow::bail!("функция «{name}» ожидает 2 аргумента, дано {}", a.len());
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
        // log(x) — десятичный; log(x, base) — по основанию.
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
                anyhow::bail!("функция «min» ожидает аргументы");
            }
            args.iter().copied().fold(f64::INFINITY, f64::min)
        }
        "max" => {
            if args.is_empty() {
                anyhow::bail!("функция «max» ожидает аргументы");
            }
            args.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        }
        other => anyhow::bail!("неизвестная функция «{other}»"),
    })
}

/// Вычисляет выражение в `f64`. Чистая функция (без I/O) — тестируема напрямую.
pub fn eval(input: &str) -> Result<f64> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        anyhow::bail!("пустое выражение");
    }
    let mut parser = Parser { tokens, pos: 0 };
    let value = parser.expr()?;
    if parser.pos != parser.tokens.len() {
        anyhow::bail!("лишние токены после выражения");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

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
        // 2^3^2 = 2^(3^2) = 2^9 = 512, а не (2^3)^2 = 64.
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
    fn errors_on_bad_input() {
        assert!(eval("").is_err());
        assert!(eval("2 +").is_err());
        assert!(eval("2 ) (").is_err());
        assert!(eval("foobar(2)").is_err());
        assert!(eval("nope").is_err());
        assert!(eval("sqrt(1, 2)").is_err()); // неверная арность
        assert!(eval("@").is_err()); // неизвестный символ
        assert!(eval("2 3").is_err()); // лишние токены
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
