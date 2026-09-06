//! Incremental, allocation-free JPEG frame extraction.
//!
//! Modified from the attributed upstream implementation for this crate.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParserProgressing {
    InputBufferEmpty,
    OutputBufferFull,
    EndOfImage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JpegParserState {
    LookingForSoi,
    Marker,
    LengthHigh,
    LengthLow,
    Segment,
    Entropy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JpegParserError {
    InvalidMarker(u8),
    InvalidSegmentLength,
}

#[derive(Debug, Eq, PartialEq)]
pub struct JpegStreamParser {
    state: JpegParserState,
    input_offset: usize,
    output_offset: usize,
    segment_remaining: usize,
    marker: u8,
    saw_ff: bool,
    resume_entropy: bool,
}

impl Default for JpegStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl JpegStreamParser {
    pub const fn new() -> Self {
        Self {
            state: JpegParserState::LookingForSoi,
            input_offset: 0,
            output_offset: 0,
            segment_remaining: 0,
            marker: 0,
            saw_ff: false,
            resume_entropy: false,
        }
    }

    pub fn parse(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<ParserProgressing, JpegParserError> {
        while self.input_offset < input.len() {
            if self.state != JpegParserState::LookingForSoi && self.output_offset >= output.len() {
                return Ok(ParserProgressing::OutputBufferFull);
            }

            let byte = input[self.input_offset];
            match self.state {
                JpegParserState::LookingForSoi => {
                    self.input_offset += 1;
                    if self.saw_ff && byte == 0xd8 {
                        if output.len() < 2 {
                            self.saw_ff = false;
                            return Ok(ParserProgressing::OutputBufferFull);
                        }
                        output[0] = 0xff;
                        output[1] = 0xd8;
                        self.output_offset = 2;
                        self.state = JpegParserState::Marker;
                        self.saw_ff = false;
                    } else {
                        self.saw_ff = byte == 0xff;
                    }
                    continue;
                }
                JpegParserState::Marker => {
                    self.write(byte, output);
                    if !self.saw_ff {
                        if byte != 0xff {
                            return self.fail(JpegParserError::InvalidMarker(byte));
                        }
                        self.saw_ff = true;
                    } else {
                        match byte {
                            0xff => {}
                            0xd9 => {
                                self.finish_frame();
                                return Ok(ParserProgressing::EndOfImage);
                            }
                            0xd8 => {
                                output[0] = 0xff;
                                output[1] = 0xd8;
                                self.output_offset = 2;
                                self.saw_ff = false;
                            }
                            0x01 => self.saw_ff = false,
                            0xd0..=0xd7 => return self.fail(JpegParserError::InvalidMarker(byte)),
                            marker if Self::has_length(marker) => {
                                self.marker = marker;
                                self.resume_entropy = false;
                                self.saw_ff = false;
                                self.state = JpegParserState::LengthHigh;
                            }
                            marker => return self.fail(JpegParserError::InvalidMarker(marker)),
                        }
                    }
                }
                JpegParserState::LengthHigh => {
                    self.write(byte, output);
                    self.segment_remaining = (byte as usize) << 8;
                    self.state = JpegParserState::LengthLow;
                }
                JpegParserState::LengthLow => {
                    self.write(byte, output);
                    self.segment_remaining |= byte as usize;
                    if self.segment_remaining < 2 {
                        return self.fail(JpegParserError::InvalidSegmentLength);
                    }
                    self.segment_remaining -= 2;
                    if self.segment_remaining == 0 {
                        self.after_segment();
                    } else {
                        self.state = JpegParserState::Segment;
                    }
                }
                JpegParserState::Segment => {
                    self.write(byte, output);
                    self.segment_remaining -= 1;
                    if self.segment_remaining == 0 {
                        self.after_segment();
                    }
                }
                JpegParserState::Entropy => {
                    self.write(byte, output);
                    if self.saw_ff {
                        match byte {
                            0x00 => self.saw_ff = false,
                            0xff => {}
                            0xd0..=0xd7 => self.saw_ff = false,
                            0x01 => self.saw_ff = false,
                            0xd9 => {
                                self.finish_frame();
                                return Ok(ParserProgressing::EndOfImage);
                            }
                            0xd8 => {
                                output[0] = 0xff;
                                output[1] = 0xd8;
                                self.output_offset = 2;
                                self.state = JpegParserState::Marker;
                                self.saw_ff = false;
                            }
                            marker if Self::has_length(marker) => {
                                self.marker = marker;
                                self.resume_entropy = true;
                                self.saw_ff = false;
                                self.state = JpegParserState::LengthHigh;
                            }
                            marker => return self.fail(JpegParserError::InvalidMarker(marker)),
                        }
                    } else {
                        self.saw_ff = byte == 0xff;
                    }
                }
            }
        }
        Ok(ParserProgressing::InputBufferEmpty)
    }

    pub fn reset_input_offset(&mut self) {
        self.input_offset = 0;
    }

    pub fn reset_output_offset(&mut self) {
        self.output_offset = 0;
    }

    pub fn reset_state(&mut self) {
        self.state = JpegParserState::LookingForSoi;
        self.output_offset = 0;
        self.segment_remaining = 0;
        self.marker = 0;
        self.saw_ff = false;
        self.resume_entropy = false;
    }

    pub const fn bytes_written(&self) -> usize {
        self.output_offset
    }

    pub const fn state(&self) -> JpegParserState {
        self.state
    }

    const fn has_length(marker: u8) -> bool {
        matches!(marker, 0xc0..=0xcf | 0xda..=0xdf | 0xe0..=0xfe)
    }

    fn write(&mut self, byte: u8, output: &mut [u8]) {
        output[self.output_offset] = byte;
        self.output_offset += 1;
        self.input_offset += 1;
    }

    fn after_segment(&mut self) {
        self.state = if self.marker == 0xda || self.resume_entropy {
            JpegParserState::Entropy
        } else {
            JpegParserState::Marker
        };
        self.saw_ff = false;
        self.resume_entropy = false;
    }

    fn finish_frame(&mut self) {
        self.state = JpegParserState::LookingForSoi;
        self.segment_remaining = 0;
        self.marker = 0;
        self.saw_ff = false;
        self.resume_entropy = false;
    }

    fn fail<T>(&mut self, error: JpegParserError) -> Result<T, JpegParserError> {
        self.reset_state();
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{vec, vec::Vec};

    fn jpeg(entropy: &[u8]) -> Vec<u8> {
        let mut bytes = vec![
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x12, 0x34, 0xff, 0xda, 0x00, 0x02,
        ];
        bytes.extend_from_slice(entropy);
        bytes.extend_from_slice(&[0xff, 0xd9]);
        bytes
    }

    #[test]
    fn extracts_markers_split_across_chunks_and_preserves_stuffing() {
        let frame = jpeg(&[0x55, 0xff, 0x00, 0xd9, 0xff, 0xd0, 0x66]);
        let stream = [
            &[0x00, 0xff][..],
            &frame[1..8],
            &frame[8..frame.len() - 1],
            &[0xd9],
        ];
        let mut parser = JpegStreamParser::new();
        let mut output = [0u8; 64];
        let mut progress = ParserProgressing::InputBufferEmpty;
        for chunk in stream {
            parser.reset_input_offset();
            progress = parser.parse(chunk, &mut output).unwrap();
        }
        assert_eq!(progress, ParserProgressing::EndOfImage);
        assert_eq!(&output[..parser.bytes_written()], frame);
    }

    #[test]
    fn extracts_multiple_frames_from_one_chunk() {
        let first = jpeg(&[1]);
        let second = jpeg(&[2, 3]);
        let mut stream = first.clone();
        stream.extend_from_slice(&second);
        let mut parser = JpegStreamParser::new();
        let mut output = [0u8; 64];

        assert_eq!(
            parser.parse(&stream, &mut output),
            Ok(ParserProgressing::EndOfImage)
        );
        assert_eq!(&output[..parser.bytes_written()], first);
        parser.reset_output_offset();
        assert_eq!(
            parser.parse(&stream, &mut output),
            Ok(ParserProgressing::EndOfImage)
        );
        assert_eq!(&output[..parser.bytes_written()], second);
    }

    #[test]
    fn reports_bounded_destination_without_consuming_more_input() {
        let frame = jpeg(&[1, 2, 3, 4]);
        let mut parser = JpegStreamParser::new();
        let mut small = [0u8; 5];
        assert_eq!(
            parser.parse(&frame, &mut small),
            Ok(ParserProgressing::OutputBufferFull)
        );
        assert_eq!(parser.bytes_written(), small.len());
        parser.reset_state();
        parser.reset_input_offset();
        let mut output = [0u8; 64];
        assert_eq!(
            parser.parse(&frame, &mut output),
            Ok(ParserProgressing::EndOfImage)
        );
    }

    #[test]
    fn rejects_bad_lengths_then_resynchronizes() {
        let good = jpeg(&[]);
        let mut stream = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x01];
        stream.extend_from_slice(&good);
        let mut parser = JpegStreamParser::new();
        let mut output = [0u8; 64];
        assert_eq!(
            parser.parse(&stream, &mut output),
            Err(JpegParserError::InvalidSegmentLength)
        );
        assert_eq!(
            parser.parse(&stream, &mut output),
            Ok(ParserProgressing::EndOfImage)
        );
        assert_eq!(&output[..parser.bytes_written()], good);
    }

    #[test]
    fn nested_soi_resynchronizes_to_the_new_frame() {
        let good = jpeg(&[7, 8]);
        let mut stream = vec![0xff, 0xd8];
        stream.extend_from_slice(&good);
        let mut parser = JpegStreamParser::new();
        let mut output = [0u8; 64];

        assert_eq!(
            parser.parse(&stream, &mut output),
            Ok(ParserProgressing::EndOfImage)
        );
        assert_eq!(&output[..parser.bytes_written()], good);
    }
}
