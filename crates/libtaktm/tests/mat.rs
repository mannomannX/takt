//! Matrizen (3.11): die Numerik gegen bekannte Ergebnisse, `inv`·`A` = I
//! bis auf die Rundung der `fma`-Kette, singulaer ist singulaer.

use libtaktm::mat::{self, Singular};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * b.abs().max(1.0)
}

#[test]
fn products_and_transposes() {
    let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2x3
    let b = [7.0, 8.0, 9.0, 10.0, 11.0, 12.0]; // 3x2
    let mut out = [0.0; 4];
    mat::mul(&a, &b, 2, 3, 2, &mut out);
    assert_eq!(out, [58.0, 64.0, 139.0, 154.0]);
    let mut t = [0.0; 6];
    mat::transpose(&a, 2, 3, &mut t);
    assert_eq!(t, [1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
    let mut s = [0.0; 4];
    mat::add(&out, &out, &mut s);
    assert_eq!(s, [116.0, 128.0, 278.0, 308.0]);
    mat::scale(&out, 0.5, &mut s);
    assert_eq!(s, [29.0, 32.0, 69.5, 77.0]);
}

#[test]
fn the_inverse_times_the_matrix_is_the_identity() {
    let a = [4.0, 7.0, 2.0, 3.0, 6.0, 1.0, 2.0, 5.0, 3.0];
    let (mut inv, mut scratch, mut perm) = ([0.0; 9], [0.0; 9], [0usize; 3]);
    mat::inv(&a, 3, &mut inv, &mut scratch, &mut perm).expect("regulaer");
    let mut id = [0.0; 9];
    mat::mul(&a, &inv, 3, 3, 3, &mut id);
    for i in 0..3 {
        for j in 0..3 {
            assert!(close(id[i * 3 + j], if i == j { 1.0 } else { 0.0 }), "{id:?}");
        }
    }
    assert!(close(mat::det(&a, 3, &mut scratch, &mut perm), 4.0 * 13.0 - 7.0 * 7.0 + 2.0 * 3.0), "det");
}

#[test]
fn solve_agrees_with_the_inverse_and_singular_is_an_error() {
    let a = [2.0, 1.0, 1.0, 3.0];
    let b = [1.0, 2.0];
    let (mut x, mut scratch, mut perm) = ([0.0; 2], [0.0; 4], [0usize; 2]);
    mat::solve(&a, 2, &b, 1, &mut x, &mut scratch, &mut perm).expect("regulaer");
    assert!(close(x[0], 0.2) && close(x[1], 0.6), "{x:?}");
    let s = [1.0, 2.0, 2.0, 4.0];
    assert_eq!(mat::inv(&s, 2, &mut [0.0; 4], &mut scratch, &mut perm), Err(Singular));
    assert_eq!(mat::det(&s, 2, &mut scratch, &mut perm), 0.0);
}

#[test]
fn cholesky_of_a_positive_definite_matrix() {
    let a = [4.0, 2.0, 2.0, 3.0];
    let mut l = [0.0; 4];
    mat::cholesky(&a, 2, &mut l).expect("positiv definit");
    assert_eq!(l, [2.0, 0.0, 1.0, 2.0f64.sqrt()]);
    assert_eq!(mat::cholesky(&[1.0, 2.0, 2.0, 1.0], 2, &mut l), None);
}

#[test]
fn single_precision_takes_the_same_path() {
    let a = [2.0f32, 1.0, 1.0, 3.0];
    let (mut inv, mut scratch, mut perm) = ([0.0f32; 4], [0.0f32; 4], [0usize; 2]);
    mat::inv(&a, 2, &mut inv, &mut scratch, &mut perm).expect("regulaer");
    assert_eq!(inv, [0.6, -0.2, -0.2, 0.4]);
    assert_eq!(mat::det(&a, 2, &mut scratch, &mut perm), 5.0);
}
