//! Small dense linear algebra used by `stats` and `risk`.
//!
//! Matrices are `Vec<Vec<f64>>` in row-major order (`a[i][j]` = row `i`,
//! column `j`), which matches the public API shapes. Sizes here are tiny
//! (regressors, portfolio assets), so clarity beats blocking.

// Triangular-matrix algorithms index several matrices with the same loop
// counters; iterator rewrites obscure the textbook formulas.
#![allow(clippy::needless_range_loop)]

use crate::error::{AnalyticsError, Result};

/// Validates that `a` is a square matrix of side `n` with finite entries.
pub(crate) fn check_square(a: &[Vec<f64>], n: usize, what: &str) -> Result<()> {
    if a.len() != n {
        return Err(AnalyticsError::LengthMismatch { expected: n, got: a.len() });
    }
    for row in a {
        if row.len() != n {
            return Err(AnalyticsError::InvalidInput(format!("{what} must be {n}x{n}")));
        }
        if row.iter().any(|v| !v.is_finite()) {
            return Err(AnalyticsError::InvalidInput(format!("{what} has non-finite entries")));
        }
    }
    Ok(())
}

/// Lower-triangular Cholesky factor `L` with `L·Lᵀ = a` for a symmetric
/// positive **semi**-definite matrix.
///
/// Zero pivots (within `1e-12 ×` the largest diagonal entry) are accepted and
/// produce a zero column, so covariance matrices of perfectly correlated or
/// zero-variance assets still factor. A clearly negative pivot or an
/// asymmetric matrix is rejected.
pub(crate) fn cholesky_psd(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>> {
    let n = a.len();
    check_square(a, n, "covariance matrix")?;
    let scale = (0..n).map(|i| a[i][i].abs()).fold(0.0_f64, f64::max);
    let sym_tol = 1e-10 * scale.max(f64::MIN_POSITIVE);
    for i in 0..n {
        for j in 0..i {
            if (a[i][j] - a[j][i]).abs() > sym_tol {
                return Err(AnalyticsError::InvalidInput("covariance matrix is not symmetric".into()));
            }
        }
    }
    let piv_tol = 1e-12 * scale;
    let mut l = vec![vec![0.0; n]; n];
    for j in 0..n {
        let d = a[j][j] - l[j][..j].iter().map(|v| v * v).sum::<f64>();
        if d < -piv_tol.max(1e-300) {
            return Err(AnalyticsError::Singular);
        }
        if d <= piv_tol {
            // Semi-definite direction: the column stays zero.
            continue;
        }
        let ljj = d.sqrt();
        l[j][j] = ljj;
        for i in (j + 1)..n {
            let dot: f64 = l[i][..j].iter().zip(&l[j][..j]).map(|(x, y)| x * y).sum();
            l[i][j] = (a[i][j] - dot) / ljj;
        }
    }
    Ok(l)
}

/// Least-squares fit of `y ≈ X·β` by Householder QR with column scaling.
pub(crate) struct LeastSquares {
    pub beta: Vec<f64>,
    /// Diagonal of `(XᵀX)⁻¹`, used for coefficient standard errors.
    pub xtx_inv_diag: Vec<f64>,
}

/// Solves the least-squares problem for the `n × p` design matrix given as
/// `p` columns of length `n` (`n ≥ p`).
///
/// Columns are scaled to unit norm before factoring so the rank test is
/// scale-free; a column whose `|R_kk|` falls below `1e-10` (relative) makes
/// the design rank-deficient and returns [`AnalyticsError::Singular`].
pub(crate) fn least_squares(columns: &[Vec<f64>], y: &[f64]) -> Result<LeastSquares> {
    let p = columns.len();
    let n = y.len();
    if p == 0 || n < p {
        return Err(AnalyticsError::InsufficientData { needed: p.max(1), got: n });
    }
    // Scale columns to unit Euclidean norm.
    let mut norms = Vec::with_capacity(p);
    let mut a: Vec<Vec<f64>> = Vec::with_capacity(p);
    for col in columns {
        let norm = col.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm == 0.0 || !norm.is_finite() {
            return Err(AnalyticsError::Singular);
        }
        norms.push(norm);
        a.push(col.iter().map(|v| v / norm).collect());
    }
    let mut b = y.to_vec();
    let mut r_diag = vec![0.0; p];
    let mut v = vec![0.0; n];
    for k in 0..p {
        let norm = a[k][k..].iter().map(|x| x * x).sum::<f64>().sqrt();
        let alpha = if a[k][k] > 0.0 { -norm } else { norm };
        let m = n - k;
        v[..m].copy_from_slice(&a[k][k..]);
        v[0] -= alpha;
        let v_norm2: f64 = v[..m].iter().map(|x| x * x).sum();
        if v_norm2 > 0.0 {
            for col in a.iter_mut().skip(k) {
                let s: f64 = v[..m].iter().zip(&col[k..]).map(|(x, y)| x * y).sum();
                let f = 2.0 * s / v_norm2;
                for (c, vi) in col[k..].iter_mut().zip(&v[..m]) {
                    *c -= f * vi;
                }
            }
            let s: f64 = v[..m].iter().zip(&b[k..]).map(|(x, y)| x * y).sum();
            let f = 2.0 * s / v_norm2;
            for (c, vi) in b[k..].iter_mut().zip(&v[..m]) {
                *c -= f * vi;
            }
        }
        r_diag[k] = a[k][k];
    }
    // Columns have unit norm, so |R_kk| ≤ 1 and the threshold is relative.
    if r_diag.iter().any(|d| d.abs() < 1e-10) {
        return Err(AnalyticsError::Singular);
    }
    // R[i][j] = a[j][i] for i ≤ j. Back-substitute R·β_s = (Qᵀy)[..p].
    let r = |i: usize, j: usize| a[j][i];
    let mut beta_s = vec![0.0; p];
    for i in (0..p).rev() {
        let s: f64 = ((i + 1)..p).map(|j| r(i, j) * beta_s[j]).sum();
        beta_s[i] = (b[i] - s) / r(i, i);
    }
    // R⁻¹ (upper triangular), column by column.
    let mut r_inv = vec![vec![0.0; p]; p];
    for j in 0..p {
        r_inv[j][j] = 1.0 / r(j, j);
        for i in (0..j).rev() {
            let s: f64 = ((i + 1)..=j).map(|k| r(i, k) * r_inv[k][j]).sum();
            r_inv[i][j] = -s / r(i, i);
        }
    }
    // (XᵀX)⁻¹ for scaled columns = R⁻¹R⁻ᵀ; diag_k = Σ_j R⁻¹[k][j]².
    // Unscale: β = β_s / norm, var factor = diag / norm².
    let beta = beta_s.iter().zip(&norms).map(|(b, n)| b / n).collect();
    let xtx_inv_diag = (0..p)
        .map(|k| r_inv[k][k..].iter().map(|x| x * x).sum::<f64>() / (norms[k] * norms[k]))
        .collect();
    Ok(LeastSquares { beta, xtx_inv_diag })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cholesky_known_matrix() {
        // A = [[4,12,-16],[12,37,-43],[-16,-43,98]] → L = [[2,0,0],[6,1,0],[-8,5,3]].
        let a = vec![vec![4.0, 12.0, -16.0], vec![12.0, 37.0, -43.0], vec![-16.0, -43.0, 98.0]];
        let l = cholesky_psd(&a).unwrap();
        let want = [[2.0, 0.0, 0.0], [6.0, 1.0, 0.0], [-8.0, 5.0, 3.0]];
        for i in 0..3 {
            for j in 0..3 {
                assert!((l[i][j] - want[i][j]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn cholesky_accepts_semidefinite_rejects_indefinite() {
        // Perfectly correlated assets: rank 1.
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let l = cholesky_psd(&a).unwrap();
        assert!((l[0][0] - 1.0).abs() < 1e-15 && (l[1][0] - 2.0).abs() < 1e-15 && l[1][1] == 0.0);
        let bad = vec![vec![1.0, 2.0], vec![2.0, 1.0]];
        assert_eq!(cholesky_psd(&bad), Err(AnalyticsError::Singular));
        let asym = vec![vec![1.0, 0.5], vec![0.1, 1.0]];
        assert!(matches!(cholesky_psd(&asym), Err(AnalyticsError::InvalidInput(_))));
    }

    #[test]
    fn least_squares_exact_fit() {
        let ones = vec![1.0; 4];
        let x = vec![1.0, 2.0, 3.0, 4.0];
        let y: Vec<f64> = x.iter().map(|v| 3.0 - 2.0 * v).collect();
        let ls = least_squares(&[ones, x], &y).unwrap();
        assert!((ls.beta[0] - 3.0).abs() < 1e-12);
        assert!((ls.beta[1] + 2.0).abs() < 1e-12);
    }

    #[test]
    fn least_squares_detects_collinearity() {
        let ones = vec![1.0; 4];
        let x = vec![1.0, 2.0, 3.0, 4.0];
        let x2: Vec<f64> = x.iter().map(|v| 2.0 * v + 1.0).collect();
        assert_eq!(least_squares(&[ones, x, x2], &[1.0, 2.0, 3.0, 5.0]).err(), Some(AnalyticsError::Singular));
    }
}
