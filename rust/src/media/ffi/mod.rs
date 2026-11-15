//! Bindings das DLLs do FFmpeg, carregadas sob demanda (porte de
//! `NoOBS/rust/src/ffi`). O exe sobe mesmo sem elas; quem precisar de codec
//! recebe o erro de "falta DLL".

pub mod ffmpeg;

/// Declara uma tabela de funções de uma DLL e o `load` que resolve todas.
/// Falta de QUALQUER símbolo faz o carregamento inteiro falhar.
#[macro_export]
macro_rules! dll_api {
    (
        $(#[$meta:meta])*
        pub struct $Api:ident;
        $( $(#[$fmeta:meta])* fn $name:ident($($arg:ident : $ty:ty),* $(,)?) $(-> $ret:ty)?; )*
    ) => {
        $(#[$meta])*
        pub struct $Api {
            _lib: ::libloading::Library,
            $( $(#[$fmeta])* pub $name: unsafe extern "C" fn($($ty),*) $(-> $ret)?, )*
        }

        impl $Api {
            /// # Safety
            /// As assinaturas declaradas têm que casar com a DLL.
            pub unsafe fn load(lib: ::libloading::Library) -> Result<Self, ::libloading::Error> {
                Ok(Self {
                    $(
                        $name: *unsafe {
                            lib.get::<unsafe extern "C" fn($($ty),*) $(-> $ret)?>(concat!(stringify!($name), "\0").as_bytes())
                        }?,
                    )*
                    _lib: lib,
                })
            }
        }
    };
}

/// `*const c_char` → `String`.
///
/// # Safety
/// `p` nulo ou string C terminada em zero.
pub unsafe fn cstr_to_string(p: *const std::os::raw::c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// Pasta das DLLs: `REFLEX_FFMPEG_DIR`, a pasta do exe (o `runtime\` do projeto,
/// ou a da instalação), ou `runtime\` num ancestral.
pub fn dll_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("REFLEX_FFMPEG_DIR") {
        return Some(dir.into());
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    if dir.join(ffmpeg::LIB_AVCODEC).exists() {
        return Some(dir);
    }
    for _ in 0..5 {
        let runtime = dir.join("runtime");
        if runtime.join(ffmpeg::LIB_AVCODEC).exists() {
            return Some(runtime);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}
