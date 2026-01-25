pub(crate) fn best_suggestion(token: &str, candidates: &[String]) -> Option<String> {
    let mut best_prefix: Option<&String> = None;
    for candidate in candidates {
        if candidate.starts_with(token) {
            best_prefix = match best_prefix {
                Some(current) if current.len() <= candidate.len() => Some(current),
                _ => Some(candidate),
            };
        }
    }
    if let Some(candidate) = best_prefix {
        return Some(candidate.clone());
    }
    let mut best = None;
    let mut best_dist = usize::MAX;
    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        let dist = edit_distance(token, candidate, 2);
        if dist <= 2 && dist < best_dist {
            best_dist = dist;
            best = Some(candidate.clone());
        }
    }
    best
}

fn edit_distance(a: &str, b: &str, max: usize) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let alen = a_bytes.len();
    let blen = b_bytes.len();
    if alen == 0 {
        return blen;
    }
    if blen == 0 {
        return alen;
    }
    let mut prev: Vec<usize> = (0..=blen).collect();
    let mut cur = vec![0; blen + 1];
    for i in 1..=alen {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=blen {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            let insert = cur[j - 1] + 1;
            let delete = prev[j] + 1;
            let replace = prev[j - 1] + cost;
            let value = insert.min(delete).min(replace);
            cur[j] = value;
            if value < row_min {
                row_min = value;
            }
        }
        if row_min > max {
            return row_min;
        }
        prev.clone_from(&cur);
    }
    prev[blen]
}
