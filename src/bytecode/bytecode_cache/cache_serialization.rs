use std::{
    fs::File,
    io::{Read, Write},
};

use crate::{
    bytecode::debug_info::{EffectSummary, FunctionDebugInfo, InstructionLocation, Location},
    diagnostics::position::{Position, Span},
    runtime::{
        compiled_function::CompiledFunction,
        cons_cell::ConsCell,
        handler_descriptor::HandlerDescriptor,
        perform_descriptor::PerformDescriptor,
        value::{AdtFields, AdtValue, Value},
    },
    syntax::Identifier,
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

fn write_symbol(writer: &mut File, value: Identifier) -> std::io::Result<()> {
    write_u32(writer, value.as_u32())
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

fn read_symbol(reader: &mut File) -> Option<Identifier> {
    Some(Identifier::new(read_u32(reader)?))
}

pub(super) fn read_string(reader: &mut File) -> Option<String> {
    let len = read_u32(reader)? as usize;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

pub(super) fn write_object(writer: &mut File, obj: &Value) -> std::io::Result<()> {
    match obj {
        Value::Integer(value) => {
            writer.write_all(&[0])?;
            write_i64(writer, *value)
        }
        Value::Float(value) => {
            writer.write_all(&[1])?;
            write_f64(writer, *value)
        }
        Value::String(value) => {
            writer.write_all(&[2])?;
            write_string(writer, value)
        }
        Value::Function(func) => {
            writer.write_all(&[3])?;
            write_u16(writer, func.num_locals as u16)?;
            write_u16(writer, func.num_parameters as u16)?;
            write_u32(writer, func.instructions.len() as u32)?;
            writer.write_all(&func.instructions)?;
            write_function_debug_info(writer, func.debug_info.as_ref())
        }
        Value::Boolean(value) => {
            writer.write_all(&[4])?;
            writer.write_all(&[u8::from(*value)])
        }
        Value::None => writer.write_all(&[5]),
        Value::EmptyList => writer.write_all(&[6]),
        Value::Some(value) => {
            writer.write_all(&[7])?;
            write_object(writer, value)
        }
        Value::Left(value) => {
            writer.write_all(&[8])?;
            write_object(writer, value)
        }
        Value::Right(value) => {
            writer.write_all(&[9])?;
            write_object(writer, value)
        }
        // Tag 10 was BaseFunction (removed). Keep tag reserved for backward compat.
        Value::Array(values) => {
            writer.write_all(&[11])?;
            write_u32(writer, values.len() as u32)?;
            for value in values.iter() {
                write_object(writer, value)?;
            }
            Ok(())
        }
        Value::Tuple(values) => {
            writer.write_all(&[12])?;
            write_u32(writer, values.len() as u32)?;
            for value in values.iter() {
                write_object(writer, value)?;
            }
            Ok(())
        }
        Value::AdtUnit(name) => {
            writer.write_all(&[13])?;
            write_string(writer, name)
        }
        Value::Cons(cell) => {
            writer.write_all(&[14])?;
            write_object(writer, &cell.head)?;
            write_object(writer, &cell.tail)
        }
        Value::Adt(adt) => {
            writer.write_all(&[15])?;
            write_string(writer, &adt.constructor)?;
            write_u32(writer, adt.fields.len() as u32)?;
            for field in adt.fields.iter() {
                write_object(writer, field)?;
            }
            Ok(())
        }
        Value::HandlerDescriptor(desc) => {
            writer.write_all(&[16])?;
            write_symbol(writer, desc.effect)?;
            write_string(writer, &desc.effect_name)?;
            write_u32(writer, desc.ops.len() as u32)?;
            for (op, op_name) in desc.ops.iter().zip(desc.op_names.iter()) {
                write_symbol(writer, *op)?;
                write_string(writer, op_name)?;
            }
            writer.write_all(&[u8::from(desc.has_state)])?;
            writer.write_all(&[u8::from(desc.is_discard)])
        }
        Value::PerformDescriptor(desc) => {
            writer.write_all(&[17])?;
            write_symbol(writer, desc.effect)?;
            write_symbol(writer, desc.op)?;
            write_string(writer, &desc.effect_name)?;
            write_string(writer, &desc.op_name)
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported constant type: {}", obj.type_name()),
        )),
    }
}

pub(super) fn read_object(reader: &mut File) -> Option<Value> {
    let mut tag = [0u8; 1];
    reader.read_exact(&mut tag).ok()?;
    match tag[0] {
        0 => Some(Value::Integer(read_i64(reader)?)),
        1 => Some(Value::Float(read_f64(reader)?)),
        2 => Some(Value::String(read_string(reader)?.into())),
        3 => {
            let num_locals = read_u16(reader)? as usize;
            let num_parameters = read_u16(reader)? as usize;
            let instructions_len = read_u32(reader)? as usize;
            let mut instructions = vec![0u8; instructions_len];
            reader.read_exact(&mut instructions).ok()?;
            let debug_info = read_function_debug_info(reader);
            Some(Value::Function(std::rc::Rc::new(CompiledFunction::new(
                instructions,
                num_locals,
                num_parameters,
                debug_info,
            ))))
        }
        4 => {
            let mut value = [0u8; 1];
            reader.read_exact(&mut value).ok()?;
            Some(Value::Boolean(value[0] != 0))
        }
        5 => Some(Value::None),
        6 => Some(Value::EmptyList),
        7 => Some(Value::Some(std::rc::Rc::new(read_object(reader)?))),
        8 => Some(Value::Left(std::rc::Rc::new(read_object(reader)?))),
        9 => Some(Value::Right(std::rc::Rc::new(read_object(reader)?))),
        10 => {
            // Tag 10 was BaseFunction (removed). Skip 1 byte for compat.
            let mut value = [0u8; 1];
            reader.read_exact(&mut value).ok()?;
            Some(Value::None)
        }
        11 => {
            let len = read_u32(reader)? as usize;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_object(reader)?);
            }
            Some(Value::Array(std::rc::Rc::new(values)))
        }
        12 => {
            let len = read_u32(reader)? as usize;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_object(reader)?);
            }
            Some(Value::Tuple(std::rc::Rc::new(values)))
        }
        13 => Some(Value::AdtUnit(std::rc::Rc::new(read_string(reader)?))),
        14 => {
            let head = read_object(reader)?;
            let tail = read_object(reader)?;
            Some(ConsCell::cons(head, tail))
        }
        15 => {
            let constructor = std::rc::Rc::new(read_string(reader)?);
            let len = read_u32(reader)? as usize;
            let mut fields = Vec::with_capacity(len);
            for _ in 0..len {
                fields.push(read_object(reader)?);
            }
            Some(Value::Adt(std::rc::Rc::new(AdtValue {
                constructor,
                fields: AdtFields::from_vec(fields),
            })))
        }
        16 => {
            let effect = read_symbol(reader)?;
            let effect_name = read_string(reader)?.into_boxed_str();
            let len = read_u32(reader)? as usize;
            let mut ops = Vec::with_capacity(len);
            let mut op_names = Vec::with_capacity(len);
            for _ in 0..len {
                ops.push(read_symbol(reader)?);
                op_names.push(read_string(reader)?.into_boxed_str());
            }
            let mut has_state = [0u8; 1];
            reader.read_exact(&mut has_state).ok()?;
            let mut is_discard = [0u8; 1];
            reader.read_exact(&mut is_discard).ok()?;
            Some(Value::HandlerDescriptor(std::rc::Rc::new(
                HandlerDescriptor {
                    effect,
                    effect_name,
                    ops,
                    op_names,
                    has_state: has_state[0] != 0,
                    is_discard: is_discard[0] != 0,
                },
            )))
        }
        17 => Some(Value::PerformDescriptor(std::rc::Rc::new(
            PerformDescriptor {
                effect: read_symbol(reader)?,
                op: read_symbol(reader)?,
                effect_name: read_string(reader)?.into_boxed_str(),
                op_name: read_string(reader)?.into_boxed_str(),
            },
        ))),
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
            match &info.boundary_location {
                None => writer.write_all(&[0])?,
                Some(location) => {
                    writer.write_all(&[1])?;
                    write_u32(writer, location.file_id)?;
                    write_span(writer, &location.span)?;
                }
            }
            let effect_tag = match info.effect_summary {
                EffectSummary::Pure => 0u8,
                EffectSummary::Unknown => 1u8,
                EffectSummary::HasEffects => 2u8,
            };
            writer.write_all(&[effect_tag])?;
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

    let mut boundary_flag = [0u8; 1];
    reader.read_exact(&mut boundary_flag).ok()?;
    let boundary_location = if boundary_flag[0] == 0 {
        None
    } else {
        let file_id = read_u32(reader)? as u32;
        let span = read_span(reader)?;
        Some(Location { file_id, span })
    };

    let mut effect_tag = [0u8; 1];
    reader.read_exact(&mut effect_tag).ok()?;
    let effect_summary = match effect_tag[0] {
        0 => EffectSummary::Pure,
        1 => EffectSummary::Unknown,
        2 => EffectSummary::HasEffects,
        _ => EffectSummary::Unknown,
    };

    Some(
        FunctionDebugInfo::new(name, files, locations)
            .with_boundary_location(boundary_location)
            .with_effect_summary(effect_summary),
    )
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
