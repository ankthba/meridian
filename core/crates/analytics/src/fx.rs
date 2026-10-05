//! FX cross rates.

/// Cross-rate matrix from USD quotes.
///
/// `usd_rates[i] = (ccy_i, units of ccy_i per 1 USD)` (include `("USD", 1.0)`
/// to get USD rows/columns). Cell `[i][j]` is the price of 1 unit of `ccy_i`
/// expressed in `ccy_j`, i.e. `rate_j / rate_i`; the diagonal is 1. Rows and
/// columns of a currency with a non-positive or non-finite rate are `NaN`.
/// Order follows the input; the currency codes are not interpreted.
pub fn cross_rate_matrix(usd_rates: &[(String, f64)]) -> Vec<Vec<f64>> {
    let valid = |r: f64| r.is_finite() && r > 0.0;
    usd_rates
        .iter()
        .map(|&(_, ri)| {
            usd_rates
                .iter()
                .map(|&(_, rj)| {
                    if !valid(ri) || !valid(rj) {
                        f64::NAN
                    } else if ri == rj {
                        1.0
                    } else {
                        rj / ri
                    }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosses() {
        let rates = vec![("USD".to_string(), 1.0), ("EUR".to_string(), 0.9), ("JPY".to_string(), 150.0), ("BAD".to_string(), 0.0)];
        let m = cross_rate_matrix(&rates);
        assert_eq!(m.len(), 4);
        for (i, row) in m.iter().enumerate().take(3) {
            assert_eq!(row[i], 1.0);
            for (j, v) in row.iter().enumerate().take(3) {
                // Reciprocity: (i→j)·(j→i) = 1.
                assert!((v * m[j][i] - 1.0).abs() < 1e-15);
            }
        }
        // 1 EUR = 1/0.9 USD; 1 USD = 150 JPY; 1 EUR = 150/0.9 JPY.
        assert!((m[1][0] - 1.0 / 0.9).abs() < 1e-15);
        assert_eq!(m[0][2], 150.0);
        assert!((m[1][2] - 150.0 / 0.9).abs() < 1e-12);
        assert!((m[2][1] - 0.9 / 150.0).abs() < 1e-18);
        assert!(m[3].iter().all(|v| v.is_nan()));
        assert!(m[0][3].is_nan());
        assert!(cross_rate_matrix(&[]).is_empty());
    }
}
