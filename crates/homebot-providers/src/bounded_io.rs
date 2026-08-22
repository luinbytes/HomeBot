use tokio::io::{AsyncBufRead, AsyncBufReadExt};

#[derive(Debug, thiserror::Error)]
pub(crate) enum BoundedLineError {
    #[error("input/output error: {0}")]
    Io(std::io::Error),
    #[error("input was not valid UTF-8")]
    InvalidUtf8,
    #[error("line exceeded its byte limit")]
    TooLong,
}

pub(crate) async fn read_line_bounded<R>(
    reader: &mut R,
    line: &mut String,
    limit: usize,
) -> Result<usize, BoundedLineError>
where
    R: AsyncBufRead + Unpin,
{
    line.clear();
    let mut bytes = Vec::with_capacity(limit.min(8 * 1024));
    loop {
        let available = reader.fill_buf().await.map_err(BoundedLineError::Io)?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(0);
            }
            break;
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if bytes.len().saturating_add(take) > limit {
            return Err(BoundedLineError::TooLong);
        }
        let terminated = available[take - 1] == b'\n';
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if terminated {
            break;
        }
    }
    let read = bytes.len();
    *line = String::from_utf8(bytes).map_err(|_| BoundedLineError::InvalidUtf8)?;
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::{BoundedLineError, read_line_bounded};
    use tokio::io::BufReader;

    #[tokio::test]
    async fn rejects_unterminated_input_before_growing_past_limit() {
        let input = vec![b'x'; 32 * 1024];
        let mut reader = BufReader::with_capacity(512, input.as_slice());
        let mut line = String::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 1_024).await,
            Err(BoundedLineError::TooLong)
        ));
        assert!(line.is_empty());
    }

    #[tokio::test]
    async fn preserves_valid_line_and_eof_semantics() -> Result<(), Box<dyn std::error::Error>> {
        let mut reader = BufReader::new(b"ready\n".as_slice());
        let mut line = String::new();
        assert_eq!(read_line_bounded(&mut reader, &mut line, 32).await?, 6);
        assert_eq!(line, "ready\n");
        assert_eq!(read_line_bounded(&mut reader, &mut line, 32).await?, 0);
        Ok(())
    }
}
