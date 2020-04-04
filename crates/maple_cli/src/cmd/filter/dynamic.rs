use super::*;
use extracted_fzy::match_and_score_with_positions;
use fuzzy_filter::FuzzyMatchedLineInfo;
use fuzzy_matcher::skim::fuzzy_indices;
use rayon::slice::ParallelSliceMut;
use std::io::{self, BufRead};
use std::time::{Duration, Instant};

/// The constant to define the length of `top_` queues.
const ITEMS_TO_SHOW: usize = 30;

const MAX_IDX: usize = ITEMS_TO_SHOW - 1;

/// Refresh the top filtered results per 200 ms.
const UPDATE_INTERVAL: Duration = Duration::from_millis(300);

trait Insert<T> {
    fn pop_and_insert(&mut self, idx: usize, value: T);
}

impl<T: Copy> Insert<T> for [T; ITEMS_TO_SHOW] {
    fn pop_and_insert(&mut self, idx: usize, value: T) {
        if idx < MAX_IDX {
            self.copy_within(idx..MAX_IDX, idx + 1);
            self[idx] = value;
        } else {
            self[MAX_IDX] = value;
        }
    }
}

/// Combine json and println macro.
///
/// Neovim needs Content-length info when using stdio-based communication.
macro_rules! print_json_with_length {
  ( $( $field:expr ),+ ) => {
    {
      let msg = serde_json::json!({ $(stringify!($field): $field,)* });
      if let Ok(s) = serde_json::to_string(&msg) {
          println!("Content-length: {}\n\n{}", s.len(), s);
      }
    }
  }
}

/// This macro is a special thing for [`dyn_collect_all`] and [`dyn_collect_number`].
macro_rules! insert_both {
            // This macro pushes all things into buffer, pops one worst item from each top queue
            // and then inserts all things into `top_` queues.
            (pop; $index:expr, $score:expr, $text:expr, $indices:expr => $buffer:expr, $top_results:expr, $top_scores:expr) => {{
                match $index {
                    // If index is last possible, then the worst item is better than this we want to push in,
                    // and we do nothing.
                    Some(MAX_IDX) => $buffer.push(($text, $score, $indices)),
                    // Else, one item gets popped from the queue
                    // and other is inserted.
                    Some(idx) => {
                        insert_both!(idx + 1, $score, $text, $indices => $buffer, $top_results, $top_scores);
                    }
                    None => {
                        insert_both!(0, $score, $text, $indices => $buffer, $top_results, $top_scores);
                    }
                }
            }};

            // This macro pushes all things into buffer and inserts all things into
            // `top_` queues.
            ($index:expr, $score:expr, $text:expr, $indices:expr => $buffer:expr, $top_results:expr, $top_scores:expr) => {{
                $buffer.push(($text, $score, $indices));
                $top_results.pop_and_insert($index, $buffer.len() - 1);
                $top_scores.pop_and_insert($index, $score);
            }};
}

type SelectedTopItemsInfo = (usize, [i64; ITEMS_TO_SHOW], [usize; ITEMS_TO_SHOW]);

/// Returns Ok if all items in the iterator has been processed.
///
/// First, let's try to produce `ITEMS_TO_SHOW` items to fill the topscores.
fn select_top_items_to_show(
    buffer: &mut Vec<FuzzyMatchedLineInfo>,
    iter: &mut impl Iterator<Item = FuzzyMatchedLineInfo>,
) -> std::result::Result<usize, SelectedTopItemsInfo> {
    let mut top_scores: [i64; ITEMS_TO_SHOW] = [i64::min_value(); ITEMS_TO_SHOW];
    let mut top_results: [usize; ITEMS_TO_SHOW] = [usize::min_value(); ITEMS_TO_SHOW];

    let mut total = 0;
    let res = iter.try_for_each(|(text, score, indices)| {
        let idx = match find_best_score_idx(&top_scores, score) {
            Some(idx) => idx + 1,
            None => 0,
        };

        insert_both!(idx, score, text, indices => buffer, top_results, top_scores);

        // Stop iterating after `ITEMS_TO_SHOW` iterations.
        total += 1;
        if total == ITEMS_TO_SHOW {
            Err(())
        } else {
            Ok(())
        }
    });

    if res.is_ok() {
        Ok(total)
    } else {
        Err((total, top_scores, top_results))
    }
}

/// Returns the index of best score in `top_scores`.
///
/// Best results are stored in front, the bigger the better.
#[inline]
fn find_best_score_idx(top_scores: &[i64; ITEMS_TO_SHOW], score: i64) -> Option<usize> {
    top_scores
        .iter()
        .enumerate()
        .rev() // .rev(), because worse items are at the end.
        .find(|&(_, &other_score)| other_score > score)
        .map(|(idx, _)| idx)
}

/// Returns the new freshed time when the new top scored items are sent to the client.
///
/// Printing to stdout is to send the printed content to the client.
fn try_notify_top_results(
    enable_icon: bool,
    total: usize,
    past: &Instant,
    top_results_len: usize,
    top_results: &[usize; ITEMS_TO_SHOW],
    buffer: &[FuzzyMatchedLineInfo],
) -> std::result::Result<Instant, ()> {
    if total % 16 == 0 {
        let now = Instant::now();
        if now > *past + UPDATE_INTERVAL {
            let mut indices = Vec::with_capacity(top_results_len);
            let mut lines = Vec::with_capacity(top_results_len);
            for &idx in top_results.iter() {
                let (text, _, idxs) = std::ops::Index::index(buffer, idx);
                indices.push(idxs);
                let text = if enable_icon {
                    prepend_icon(&text)
                } else {
                    text.clone()
                };
                lines.push(text);
            }

            print_json_with_length!(total, lines, indices);

            return Ok(now);
        }
    }
    Err(())
}

/// To get dynamic updates, not so much should be changed, actually.
/// First: instead of collecting iterator into vector, this iterator
/// should be `for_each`ed or something like this.
/// Second: while iterator is `for_each`ed, its results are collected
/// into some collection, and `total` is increased by one for each iteration.
/// At some points of iteration that collection gets printed and voila!
///
/// Though it sounds easy, there's one pitfalls:
/// `par_iter` iteration should use atomic `total`, because, well, it's parallel.
/// And some rough edges: if there's too much results, sorting and json+print
/// could take too much time. Same problem for too big `number`.
///
/// So, to get dynamic results, I'm gonna use VecDeque with little constant space.
/// But there's a problem with `par_iter` again, as there should be mutexed access to the
/// VecDeque for this iterator.
///
/// So, this particular function won't work in parallel context at all.
fn dyn_collect_all(
    mut iter: impl Iterator<Item = FuzzyMatchedLineInfo>,
    enable_icon: bool,
) -> Vec<FuzzyMatchedLineInfo> {
    let mut buffer = Vec::with_capacity({
        let (low, high) = iter.size_hint();
        high.unwrap_or(low)
    });

    let should_return = select_top_items_to_show(&mut buffer, &mut iter);

    let (mut total, mut top_scores, mut top_results) = match should_return {
        Ok(_) => return buffer,
        Err((t, top_scores, top_results)) => (t, top_scores, top_results),
    };

    // Now we have the full queue and can just pair `.pop_back()` with `.insert()` to keep
    // the queue with best results the same size.
    let mut past = std::time::Instant::now();
    iter.for_each(|(text, score, indices)| {
        let idx = find_best_score_idx(&top_scores, score);

        insert_both!(pop; idx, score, text, indices => buffer, top_results, top_scores);

        total = total.wrapping_add(1);

        if let Ok(now) = try_notify_top_results(
            enable_icon,
            total,
            &past,
            top_results.len(),
            &top_results,
            &buffer,
        ) {
            past = now;
        }
    });

    buffer
}

/// If you only need a `number` of elements, then you don't need to collect all
/// items produced by the iterator.
///
/// # Returns
///
/// Tuple of `(total_number_of_iterations: usize, Vec<_>)`.
/// The vector is not sorted nor truncated.
//
// Even though the current implementation isn't the most effective thing to do it,
// I think, it's just good enough. And should be more effective than full
// `collect()` into Vec on big numbers of iterations.
fn dyn_collect_number(
    mut iter: impl Iterator<Item = FuzzyMatchedLineInfo>,
    enable_icon: bool,
    number: usize,
) -> (usize, Vec<FuzzyMatchedLineInfo>) {
    // To not have problems with queues after sorting and truncating the buffer,
    // buffer has the lowest bound of `ITEMS_TO_SHOW * 2`, not `number * 2`.
    let mut buffer = Vec::with_capacity(2 * std::cmp::max(ITEMS_TO_SHOW, number));

    let should_return = select_top_items_to_show(&mut buffer, &mut iter);

    let (mut total, mut top_scores, mut top_results) = match should_return {
        Ok(t) => return (t, buffer),
        Err((t, top_scores, top_results)) => (t, top_scores, top_results),
    };

    // Now we have the full queue and can just pair `.pop_back()` with `.insert()` to keep
    // the queue with best results the same size.
    let mut past = std::time::Instant::now();
    iter.for_each(|(text, score, indices)| {
        let idx = find_best_score_idx(&top_scores, score);

        insert_both!(pop; idx, score, text, indices => buffer, top_results, top_scores);

        total += 1;

        if let Ok(now) = try_notify_top_results(
            enable_icon,
            total,
            &past,
            top_results.len(),
            &top_results,
            &buffer,
        ) {
            past = now;
        }

        if buffer.len() == buffer.capacity() {
            buffer.par_sort_unstable_by(|(_, v1, _), (_, v2, _)| v2.partial_cmp(&v1).unwrap());

            for (idx, (_, score, _)) in buffer[..ITEMS_TO_SHOW].iter().enumerate() {
                top_scores[idx] = *score;
                top_results[idx] = idx;
            }

            let half = buffer.len() / 2;
            buffer.truncate(half);
        }
    });

    (total, buffer)
}

/// Returns the ranked results after applying fuzzy filter given the query string and a list of candidates.
pub fn dyn_fuzzy_filter_and_rank<I: Iterator<Item = String>>(
    query: &str,
    source: Source<I>,
    algo: Option<Algo>,
    number: Option<usize>,
    enable_icon: bool,
    winwidth: Option<usize>,
) -> Result<()> {
    let algo = algo.unwrap_or(Algo::Fzy);

    let scorer = |line: &str| match algo {
        Algo::Skim => fuzzy_indices(line, query),
        Algo::Fzy => match_and_score_with_positions(query, line)
            .map(|(score, indices)| (score as i64, indices)),
    };

    if let Some(number) = number {
        let (total, filtered) = match source {
            Source::Stdin => dyn_collect_number(
                io::stdin().lock().lines().filter_map(|lines_iter| {
                    lines_iter.ok().and_then(|line| {
                        scorer(&line).map(|(score, indices)| (line, score, indices))
                    })
                }),
                enable_icon,
                number,
            ),
            Source::Exec(exec) => dyn_collect_number(
                std::io::BufReader::new(exec.stream_stdout()?)
                    .lines()
                    .filter_map(|lines_iter| {
                        lines_iter.ok().and_then(|line| {
                            scorer(&line).map(|(score, indices)| (line, score, indices))
                        })
                    }),
                enable_icon,
                number,
            ),
            Source::File(fpath) => dyn_collect_number(
                std::fs::read_to_string(fpath)?.lines().filter_map(|line| {
                    scorer(&line).map(|(score, indices)| (line.into(), score, indices))
                }),
                enable_icon,
                number,
            ),
            Source::List(list) => dyn_collect_number(
                list.filter_map(|line| {
                    scorer(&line).map(|(score, indices)| (line, score, indices))
                }),
                enable_icon,
                number,
            ),
        };
        let (lines, indices, truncated_map) = process_top_items(
            number,
            filtered.into_iter().take(number),
            winwidth.unwrap_or(62),
            enable_icon,
        );

        if truncated_map.is_empty() {
            print_json_with_length!(total, lines, indices);
        } else {
            print_json_with_length!(total, lines, indices, truncated_map);
        }
    } else {
        let mut filtered = match source {
            Source::Stdin => dyn_collect_all(
                io::stdin().lock().lines().filter_map(|lines_iter| {
                    lines_iter.ok().and_then(|line| {
                        scorer(&line).map(|(score, indices)| (line, score, indices))
                    })
                }),
                enable_icon,
            ),
            Source::Exec(exec) => dyn_collect_all(
                std::io::BufReader::new(exec.stream_stdout()?)
                    .lines()
                    .filter_map(|lines_iter| {
                        lines_iter.ok().and_then(|line| {
                            scorer(&line).map(|(score, indices)| (line, score, indices))
                        })
                    }),
                enable_icon,
            ),
            Source::File(fpath) => dyn_collect_all(
                std::fs::read_to_string(fpath)?.lines().filter_map(|line| {
                    scorer(line).map(|(score, indices)| (line.into(), score, indices))
                }),
                enable_icon,
            ),
            Source::List(list) => dyn_collect_all(
                list.filter_map(|line| {
                    scorer(&line).map(|(score, indices)| (line, score, indices))
                }),
                enable_icon,
            ),
        };

        filtered.par_sort_unstable_by(|(_, v1, _), (_, v2, _)| v2.partial_cmp(&v1).unwrap());

        let ranked = filtered;

        for (text, _, indices) in ranked.iter() {
            println_json!(text, indices);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // This is a very time-consuming test,
    // results of which could be proved only be inspecting stdout.
    // Definetly not something you want to run with `cargo test`.
    #[ignore]
    fn dynamic_results() {
        use std::time::{SystemTime, UNIX_EPOCH};

        const ALPHABET: [u8; 32] = [
            b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'a', b's',
            b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'z', b'x', b'c', b'v', b'b', b'n',
            b'm', b',', b'.', b' ',
        ];

        // To mock the endless randomized text, we need three numbers:
        // 1. A number of letters to change.
        // then for each such number
        // 2. Alphabet index to get a new letter,
        // 3. Changing text index to write a new letter.
        let now = SystemTime::now();
        let mut bytes: usize = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| UNIX_EPOCH.duration_since(now).unwrap())
            .as_secs() as usize;

        let mut changing_text: [u8; 16] = [ALPHABET[31]; 16];
        let mut total_lines_created: usize = 0;
        dyn_fuzzy_filter_and_rank(
            "abc",
            Source::List(
                std::iter::repeat_with(|| {
                    bytes = bytes.reverse_bits().rotate_right(3).wrapping_add(1);

                    let mut n = bytes;
                    // Number of letter to change.
                    let i = (n % 4) + 1;
                    n /= 4;
                    for _ in 0..i {
                        let text_idx = n % 16;
                        n /= 16;
                        let ab_idx = n % 32;
                        n /= 32;

                        changing_text[text_idx] = ALPHABET[ab_idx];
                    }

                    total_lines_created += 1;
                    if total_lines_created % 99999_usize.next_power_of_two() == 0 {
                        println!("Total lines created: {}", total_lines_created)
                    }

                    String::from_utf8(changing_text.as_ref().to_owned()).unwrap()
                })
                .take(usize::max_value() >> 8),
            ),
            Some(Algo::Fzy),
            Some(100),
            false,
            None,
        )
        .unwrap()
    }
}
