use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
};

const WORD: usize = size_of::<usize>();

pub fn hide() -> io::Result<()> {
    let stat = std::fs::read_to_string("/proc/self/stat")?;
    let fields: Vec<&str> = stat
        .trim_end()
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("/proc/self/stat has no command name"))?
        .1
        .split(' ')
        .collect();
    let field = |number: usize| {
        fields
            .get(number - 3)
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                io::Error::other(format!("/proc/self/stat has no address in field {number}"))
            })
    };
    let (stack, start, end) = (field(28)?, field(48)?, field(49)?);
    let mut memory = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/proc/self/mem")?;
    let mut area = vec![0; usize::try_from(end - start).map_err(io::Error::other)?];
    memory.seek(SeekFrom::Start(start))?;
    memory.read_exact(&mut area)?;
    let arguments: Vec<&[u8]> = area
        .strip_suffix(&[0])
        .ok_or_else(|| io::Error::other("the argument area was already rewritten"))?
        .split(|byte| *byte == 0)
        .collect();
    if read_word(&mut memory, stack)? != arguments.len() {
        return Err(io::Error::other(
            "the argument area does not hold the arguments exec gave",
        ));
    }
    let mut offset = start;
    for (index, argument) in arguments.iter().enumerate() {
        let slot = stack + ((index + 1) * WORD) as u64;
        if read_word(&mut memory, slot)? as u64 != offset {
            return Err(io::Error::other("argv does not point at the argument area"));
        }
        let copy = Box::leak(
            CString::new(*argument)
                .map_err(io::Error::other)?
                .into_boxed_c_str(),
        );
        memory.seek(SeekFrom::Start(slot))?;
        memory.write_all(&(copy.as_ptr() as usize).to_ne_bytes())?;
        offset += argument.len() as u64 + 1;
    }
    let shown = arguments[0].len() + 1;
    memory.seek(SeekFrom::Start(start + shown as u64))?;
    memory.write_all(&vec![0; area.len() - shown])
}

fn read_word(memory: &mut File, address: u64) -> io::Result<usize> {
    let mut word = [0; WORD];
    memory.seek(SeekFrom::Start(address))?;
    memory.read_exact(&mut word)?;
    Ok(usize::from_ne_bytes(word))
}
