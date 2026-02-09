use std::{
    fs::File,
    io::{Read, Write},
};

use crate::{
    bytecode::debug_info::{FunctionDebugInfo, InstructionLocation, Location},
    syntax::position::{Position, Span},
    runtime::{compiled_function::CompiledFunction, object::Object},
};

pub(super) fn write_u16(writer: &mut File, value: u16) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

pub(super) fn write_u32(writer: &mut File, value: u32) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_i64(writer: &mut File, value: i64) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_f64(writer: &mut File, value: f64) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

pub(super) fn write_string(writer: &mut File, value: &str) -> std::io::Result<()> {
    let bytes = value.as_bytes();
    write_u32(writer, bytes.len() as u32)?;
    writer.write_all(bytes)
}

pub(super) fn read_u16(reader: &mut File) -> Option<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf).ok()?;
    Some(u16::from_le_bytes(buf))
}

pub(super) fn read_u32(reader: &mut File) -> Option<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).ok()?;
    Some(u32::from_le_bytes(buf))
}

fn read_i64(reader: &mut File) -> Option<i64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf).ok()?;
    Some(i64::from_le_bytes(buf))
}

fn read_f64(reader: &mut File) -> Option<f64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf).ok()?;
    Some(f64::from_le_bytes(buf))
}

pub(super) fn read_string(reader: &mut File) -> Option<String> {
    let len = read_u32(reader)? as usize;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

pub(super) fn write_object(writer: &mut File, obj: &Object) -> std::io::Result<()> {
    match obj {
        Object::Integer(value) => {
            writer.write_all(&[0])?;
            write_i64(writer, *value)
        }
        Object::Float(value) => {
            writer.write_all(&[1])?;
            write_f64(writer, *value)
        }
        Object::String(value) => {
            writer.write_all(&[2])?;
            write_string(writer, value)
        }
        Object::Function(func) => {
            writer.write_all(&[3])?;
            write_u16(writer, func.num_locals as u16)?;
            write_u16(writer, func.num_parameters as u16)?;
            write_u32(writer, func.instructions.len() as u32)?;
            writer.write_all(&func.instructions)?;
            write_function_debug_info(writer, func.debug_info.as_ref())
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported constant type: {}", obj.type_name()),
        )),
    }
}

pub(super) fn read_object(reader: &mut File) -> Option<Object> {
    let mut tag = [0u8; 1];
    reader.read_exact(&mut tag).ok()?;
    match tag[0] {
        0 => Some(Object::Integer(read_i64(reader)?)),
        1 => Some(Object::Float(read_f64(reader)?)),
        2 => Some(Object::String(read_string(reader)?)),
        3 => {
            let num_locals = read_u16(reader)? as usize;
            let num_parameters = read_u16(reader)? as usize;
            let instructions_len = read_u32(reader)? as usize;
            let mut instructions = vec![0u8; instructions_len];
            reader.read_exact(&mut instructions).ok()?;
            let debug_info = read_function_debug_info(reader);
            Some(Object::Function(std::rc::Rc::new(CompiledFunction::new(
                instructions,
                num_locals,
                num_parameters,
                debug_info,
            ))))
        }
        _ => None,
    }
}

pub(super) fn write_function_debug_info(
    writer: &mut File,
    debug_info: Option<&FunctionDebugInfo>,
) -> std::io::Result<()> {
    match debug_info {
        None => writer.write_all(&[0]),
        Some(info) => {
            writer.write_all(&[1])?;
            match &info.name {
                None => writer.write_all(&[0])?,
                Some(name) => {
                    writer.write_all(&[1])?;
                    write_string(writer, name)?;
                }
            }
            write_u32(writer, info.files.len() as u32)?;
            for file in &info.files {
                write_string(writer, file)?;
            }
            write_u32(writer, info.locations.len() as u32)?;
            for entry in &info.locations {
                write_u32(writer, entry.offset as u32)?;
                match &entry.location {
                    None => writer.write_all(&[0])?,
                    Some(location) => {
                        writer.write_all(&[1])?;
                        write_u32(writer, location.file_id)?;
                        write_span(writer, &location.span)?;
                    }
                }
            }
            Ok(())
        }
    }
}

pub(super) fn read_function_debug_info(reader: &mut File) -> Option<FunctionDebugInfo> {
    let mut flag = [0u8; 1];
    reader.read_exact(&mut flag).ok()?;
    if flag[0] == 0 {
        return None;
    }

    let mut name_flag = [0u8; 1];
    reader.read_exact(&mut name_flag).ok()?;
    let name = if name_flag[0] == 0 {
        None
    } else {
        Some(read_string(reader)?)
    };

    let files_len = read_u32(reader)? as usize;
    let mut files = Vec::with_capacity(files_len);
    for _ in 0..files_len {
        files.push(read_string(reader)?);
    }

    let locations_len = read_u32(reader)? as usize;
    let mut locations = Vec::with_capacity(locations_len);
    for _ in 0..locations_len {
        let offset = read_u32(reader)? as usize;
        let mut loc_flag = [0u8; 1];
        reader.read_exact(&mut loc_flag).ok()?;
        let location = if loc_flag[0] == 0 {
            None
        } else {
            let file_id = read_u32(reader)? as u32;
            let span = read_span(reader)?;
            Some(Location { file_id, span })
        };
        locations.push(InstructionLocation { offset, location });
    }

    Some(FunctionDebugInfo::new(name, files, locations))
}

fn write_position(writer: &mut File, position: &Position) -> std::io::Result<()> {
    write_u32(writer, position.line as u32)?;
    write_u32(writer, position.column as u32)?;
    Ok(())
}

fn read_position(reader: &mut File) -> Option<Position> {
    Some(Position::new(
        read_u32(reader)? as usize,
        read_u32(reader)? as usize,
    ))
}

fn write_span(writer: &mut File, span: &Span) -> std::io::Result<()> {
    write_position(writer, &span.start)?;
    write_position(writer, &span.end)?;
    Ok(())
}

fn read_span(reader: &mut File) -> Option<Span> {
    let start = read_position(reader)?;
    let end = read_position(reader)?;
    Some(Span::new(start, end))
}
