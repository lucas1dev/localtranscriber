// Som do computador no Mac: process tap GLOBAL do Core Audio.
//
// A cpal só faz loopback quando a saída padrão não tem entrada; com um fone USB,
// Bluetooth ou AirPods como saída, ela abriria o microfone do fone. Aqui o tap
// é global (todos os processos, qualquer saída), então segue a troca de saída
// sozinho e nunca precisa ser reaberto.
//
// Passos (como o `loopback.rs` da cpal 0.18.2, mas com tap global):
// 1. `CATapDescription` mono global, sem excluir processos, sem mudo, privado;
// 2. `AudioHardwareCreateProcessTap`;
// 3. dispositivo agregado privado só com o tap (UID único, auto-start do tap);
// 4. formato da entrada do agregado (float32) e IOProc + `AudioDeviceStart`.
// O IOProc segue as regras do callback da cpal: sem trava, sem E/S, `try_send`.
use std::ffi::{c_void, CStr};
use std::mem::{size_of, MaybeUninit};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioDevicePermissionsError,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreams,
    kAudioHardwareBadPropertySizeError, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioStreamPropertyVirtualFormat, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectRemovePropertyListener, CATapDescription,
    CATapMuteBehavior,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatLinearPCM, AudioBuffer, AudioBufferList,
    AudioStreamBasicDescription, AudioTimeStamp,
};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use super::devices::{Raw, RawSink, SYSTEM_PERMISSION};
use crate::error::AppError;

/// Contador para o UID do agregado ser único em cada abertura.
static INSTANCE: AtomicU32 = AtomicU32::new(0);

const AGGREGATE_NAME: &str = "Local Transcriber (som do computador)";
/// No máximo tantos buffers não intercalados (o tap é mono: na prática, 1).
const MAX_PLANES: usize = 8;

const RATE_ADDRESS: AudioObjectPropertyAddress = AudioObjectPropertyAddress {
    mSelector: kAudioDevicePropertyNominalSampleRate,
    mScope: kAudioObjectPropertyScopeGlobal,
    mElement: kAudioObjectPropertyElementMain,
};

/// Dados do IOProc. A taxa é atualizada por um listener quando o agregado muda
/// de taxa (por exemplo, fone Bluetooth entrando em modo de chamada).
struct IoCtx {
    sink: RawSink,
    /// `'static` de propósito: o listener pode rodar numa thread do Core Audio
    /// logo depois de removido, então o ponteiro dele nunca é liberado (4 bytes
    /// por gravação).
    rate: &'static AtomicU32,
}

/// Tap global + agregado + IOProc. O drop para e desfaz tudo, na ordem:
/// para o dispositivo, destrói o IOProc, o agregado e o tap.
pub(super) struct SystemTap {
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    rate: Option<&'static AtomicU32>,
    ctx: *mut IoCtx,
}

// Só ids do Core Audio e um ponteiro para `IoCtx` (que é `Send + Sync`) de posse
// exclusiva desta struct.
unsafe impl Send for SystemTap {}

impl SystemTap {
    /// Cria o tap e o agregado e começa a mandar áudio para `sink`.
    pub fn open(sink: RawSink) -> Result<SystemTap, AppError> {
        let mut tap = SystemTap {
            tap_id: 0,
            aggregate_id: 0,
            proc_id: None,
            rate: None,
            ctx: std::ptr::null_mut(),
        };

        // 1–2. Tap global mono de todos os processos, sem silenciar a saída.
        let description = unsafe {
            let d = CATapDescription::initMonoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &NSArray::new(),
            );
            d.setMuteBehavior(CATapMuteBehavior::Unmuted);
            d.setPrivate(true);
            d
        };
        let mut tap_id: AudioObjectID = 0;
        check("criar o tap de áudio", unsafe {
            AudioHardwareCreateProcessTap(Some(&description), &mut tap_id)
        })?;
        tap.tap_id = tap_id;

        // 3. Agregado privado que só contém o tap.
        let tap_uid = unsafe { description.UUID().UUIDString() };
        let uid = aggregate_uid(std::process::id(), INSTANCE.fetch_add(1, Ordering::Relaxed));
        let properties = aggregate_properties(&tap_uid, &uid, AGGREGATE_NAME);
        // NSDictionary e CFDictionary são o mesmo objeto (toll-free bridging).
        let cf_properties: &CFDictionary =
            unsafe { &*(Retained::as_ptr(&properties) as *const CFDictionary) };
        let mut aggregate_id: AudioObjectID = 0;
        check("criar o dispositivo agregado", unsafe {
            AudioHardwareCreateAggregateDevice(cf_properties, NonNull::from(&mut aggregate_id))
        })?;
        tap.aggregate_id = aggregate_id;

        // 4. Formato da entrada do agregado.
        let stream: AudioObjectID = unsafe {
            property(
                aggregate_id,
                kAudioDevicePropertyStreams,
                kAudioObjectPropertyScopeInput,
            )
        }
        .map_err(|s| os_error("ler a entrada do agregado", s))?;
        let format: AudioStreamBasicDescription = unsafe {
            property(
                stream,
                kAudioStreamPropertyVirtualFormat,
                kAudioObjectPropertyScopeGlobal,
            )
        }
        .map_err(|s| os_error("ler o formato do agregado", s))?;
        let rate = float32_rate(&format).ok_or_else(|| {
            AppError::Internal(format!(
                "Formato inesperado do som do computador: {} Hz, formato {}, flags {:#x}, {} bits.",
                format.mSampleRate,
                status_text(format.mFormatID as i32),
                format.mFormatFlags,
                format.mBitsPerChannel
            ))
        })?;

        let rate: &'static AtomicU32 = Box::leak(Box::new(AtomicU32::new(rate)));
        tap.ctx = Box::into_raw(Box::new(IoCtx { sink, rate }));
        let mut proc_id: AudioDeviceIOProcID = None;
        check("registrar a leitura do áudio", unsafe {
            AudioDeviceCreateIOProcID(
                aggregate_id,
                Some(io_proc),
                tap.ctx.cast(),
                NonNull::from(&mut proc_id),
            )
        })?;
        tap.proc_id = proc_id;

        check("acompanhar a taxa de amostragem", unsafe {
            AudioObjectAddPropertyListener(
                aggregate_id,
                NonNull::from(&RATE_ADDRESS),
                Some(rate_listener),
                (rate as *const AtomicU32).cast_mut().cast(),
            )
        })?;
        tap.rate = Some(rate);

        check("iniciar a captura", unsafe {
            AudioDeviceStart(aggregate_id, tap.proc_id)
        })?;
        Ok(tap)
    }
}

impl Drop for SystemTap {
    fn drop(&mut self) {
        // Os status são ignorados: não há o que fazer no drop.
        unsafe {
            if self.proc_id.is_some() {
                let _ = AudioDeviceStop(self.aggregate_id, self.proc_id);
                let _ = AudioDeviceDestroyIOProcID(self.aggregate_id, self.proc_id);
            }
            if let Some(rate) = self.rate {
                let _ = AudioObjectRemovePropertyListener(
                    self.aggregate_id,
                    NonNull::from(&RATE_ADDRESS),
                    Some(rate_listener),
                    (rate as *const AtomicU32).cast_mut().cast(),
                );
            }
            if self.aggregate_id != 0 {
                let _ = AudioHardwareDestroyAggregateDevice(self.aggregate_id);
            }
            if self.tap_id != 0 {
                let _ = AudioHardwareDestroyProcessTap(self.tap_id);
            }
            if !self.ctx.is_null() {
                // Solta o remetente da fila bruta: a thread de conversão termina.
                drop(Box::from_raw(self.ctx));
            }
        }
    }
}

/// IOProc do agregado. Roda na thread de tempo real do Core Audio.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    let Some(ctx) = (client as *const IoCtx).as_ref() else {
        return 0;
    };
    let list = input.as_ptr();
    let count = (*list).mNumberBuffers as usize;
    let first = std::ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>();
    let buffers = std::slice::from_raw_parts(first, count);
    if let Some((samples, channels)) = read_buffers(buffers) {
        ctx.sink.push(Raw {
            samples,
            rate: ctx.rate.load(Ordering::Relaxed),
            channels,
        });
    }
    0
}

/// Listener da taxa nominal do agregado: guarda a taxa nova para o IOProc.
unsafe extern "C-unwind" fn rate_listener(
    object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> i32 {
    let Some(rate) = (client as *const AtomicU32).as_ref() else {
        return 0;
    };
    if let Ok(now) = property::<f64>(
        object,
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
    ) {
        if now >= 1.0 {
            rate.store(now.round() as u32, Ordering::Relaxed);
        }
    }
    0
}

/// Lê os buffers do IOProc como amostras intercaladas e o número de canais.
unsafe fn read_buffers(buffers: &[AudioBuffer]) -> Option<(Vec<f32>, u16)> {
    unsafe fn plane(b: &AudioBuffer) -> Option<&[f32]> {
        if b.mData.is_null() {
            return None;
        }
        let len = b.mDataByteSize as usize / size_of::<f32>();
        Some(std::slice::from_raw_parts(b.mData as *const f32, len))
    }
    let (samples, channels) = match buffers {
        [] => return None,
        [one] => (
            plane(one)?.to_vec(),
            u16::try_from(one.mNumberChannels).ok()?,
        ),
        many => {
            // Não intercalado: um canal por buffer.
            let n = many.len().min(MAX_PLANES);
            let mut planes: [&[f32]; MAX_PLANES] = [&[]; MAX_PLANES];
            for (slot, b) in planes.iter_mut().zip(&many[..n]) {
                *slot = plane(b)?;
            }
            (interleave_planar(&planes[..n]), n as u16)
        }
    };
    (!samples.is_empty() && channels > 0).then_some((samples, channels))
}

/// Lê uma propriedade de tamanho fixo de um objeto do Core Audio. Para listas,
/// devolve o primeiro item.
unsafe fn property<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Result<T, i32> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut value = MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    let status = AudioObjectGetPropertyData(
        object,
        NonNull::from(&address),
        0,
        std::ptr::null(),
        NonNull::from(&mut size),
        NonNull::new_unchecked(value.as_mut_ptr().cast()),
    );
    if status != 0 {
        return Err(status);
    }
    if size as usize != size_of::<T>() {
        return Err(kAudioHardwareBadPropertySizeError);
    }
    Ok(value.assume_init())
}

fn check(step: &str, status: i32) -> Result<(), AppError> {
    if status == 0 {
        Ok(())
    } else {
        Err(os_error(step, status))
    }
}

/// OSStatus de falha em erro do app. Permissão negada usa a mensagem de ajuda.
fn os_error(step: &str, status: i32) -> AppError {
    if status == kAudioDevicePermissionsError {
        return AppError::User(SYSTEM_PERMISSION.into());
    }
    AppError::Internal(format!(
        "Não foi possível capturar o som do computador: falha ao {step} ({}).",
        status_text(status)
    ))
}

/// `OSStatus 1852797029 'nope'`; o código de quatro letras só quando é legível.
fn status_text(status: i32) -> String {
    let bytes = status.to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        format!("OSStatus {status} '{}'", String::from_utf8_lossy(&bytes))
    } else {
        format!("OSStatus {status}")
    }
}

/// A taxa, se o formato é PCM float de 32 bits (o que o tap entrega).
fn float32_rate(format: &AudioStreamBasicDescription) -> Option<u32> {
    let ok = format.mFormatID == kAudioFormatLinearPCM
        && format.mFormatFlags & kAudioFormatFlagIsFloat != 0
        && format.mBitsPerChannel == 32
        && format.mChannelsPerFrame > 0
        && format.mSampleRate >= 1.0;
    ok.then(|| format.mSampleRate.round() as u32)
}

fn aggregate_uid(pid: u32, instance: u32) -> String {
    format!("com.localtranscriber.systemtap.{pid}.{instance}")
}

fn interleave_planar(planes: &[&[f32]]) -> Vec<f32> {
    let frames = planes.iter().map(|p| p.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(frames * planes.len());
    for i in 0..frames {
        for p in planes {
            out.push(p[i]);
        }
    }
    out
}

fn ns_key(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().unwrap_or_default())
}

/// Descrição do agregado: privado, só com o tap, que começa sozinho.
fn aggregate_properties(
    tap_uid: &NSString,
    uid: &str,
    name: &str,
) -> Retained<NSDictionary<NSString, AnyObject>> {
    let yes = NSNumber::new_bool(true);
    let sub_tap = NSDictionary::<NSString, AnyObject>::from_slices(
        &[
            &*ns_key(kAudioSubTapUIDKey),
            &*ns_key(kAudioSubTapDriftCompensationKey),
        ],
        &[tap_uid.as_ref(), yes.as_ref()],
    );
    let taps = NSArray::from_slice(&[&*sub_tap]);
    let uid = NSString::from_str(uid);
    let name = NSString::from_str(name);
    NSDictionary::from_slices(
        &[
            &*ns_key(kAudioAggregateDeviceNameKey),
            &*ns_key(kAudioAggregateDeviceUIDKey),
            &*ns_key(kAudioAggregateDeviceTapListKey),
            &*ns_key(kAudioAggregateDeviceTapAutoStartKey),
            &*ns_key(kAudioAggregateDeviceIsPrivateKey),
        ],
        &[
            name.as_ref(),
            uid.as_ref(),
            taps.as_ref(),
            yes.as_ref(),
            yes.as_ref(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_audio::{
        kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey,
        kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
        kAudioAggregateDeviceUIDKey, kAudioSubTapUIDKey,
    };
    use objc2_foundation::{NSArray, NSNumber};

    fn key(k: &std::ffi::CStr) -> Retained<NSString> {
        NSString::from_str(k.to_str().unwrap())
    }

    #[test]
    fn status_text_shows_number_and_fourcc() {
        assert_eq!(status_text(0x6e6f7065), "OSStatus 1852797029 'nope'");
        assert_eq!(status_text(-50), "OSStatus -50");
    }

    #[test]
    fn float32_rate_accepts_only_float32_pcm() {
        let mut f = AudioStreamBasicDescription {
            mSampleRate: 48_000.0,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        assert_eq!(float32_rate(&f), Some(48_000));
        f.mBitsPerChannel = 16;
        assert_eq!(float32_rate(&f), None);
        f.mBitsPerChannel = 32;
        f.mFormatFlags = 0;
        assert_eq!(float32_rate(&f), None);
    }

    #[test]
    fn permission_status_uses_help_message() {
        match os_error("x", kAudioDevicePermissionsError) {
            AppError::User(m) => assert_eq!(m, SYSTEM_PERMISSION),
            other => panic!("{other:?}"),
        }
        match os_error("criar o tap de áudio", 0x6e6f7065) {
            AppError::Internal(m) => assert!(m.contains("OSStatus 1852797029 'nope'"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn aggregate_uid_is_unique_per_instance() {
        assert_ne!(aggregate_uid(10, 0), aggregate_uid(10, 1));
        assert_ne!(aggregate_uid(10, 0), aggregate_uid(11, 0));
        assert!(aggregate_uid(10, 0).starts_with("com.localtranscriber."));
    }

    #[test]
    fn interleave_planar_merges_channels() {
        assert_eq!(
            interleave_planar(&[&[1.0, 2.0], &[3.0, 4.0]]),
            vec![1.0, 3.0, 2.0, 4.0]
        );
        // Planos de tamanhos diferentes: usa o menor.
        assert_eq!(interleave_planar(&[&[1.0, 2.0, 9.0], &[3.0, 4.0]]).len(), 4);
        assert_eq!(interleave_planar(&[&[5.0, 6.0]]), vec![5.0, 6.0]);
    }

    #[test]
    fn aggregate_properties_has_private_autostart_tap() {
        let tap_uid = NSString::from_str("TAP-UID");
        let dict = aggregate_properties(&tap_uid, "com.localtranscriber.x.1", "Nome");
        let get = |k: &std::ffi::CStr| dict.objectForKey(&key(k)).expect("chave ausente");

        let uid = get(kAudioAggregateDeviceUIDKey)
            .downcast::<NSString>()
            .unwrap();
        assert_eq!(uid.to_string(), "com.localtranscriber.x.1");
        let name = get(kAudioAggregateDeviceNameKey)
            .downcast::<NSString>()
            .unwrap();
        assert_eq!(name.to_string(), "Nome");
        for flag in [
            kAudioAggregateDeviceIsPrivateKey,
            kAudioAggregateDeviceTapAutoStartKey,
        ] {
            let n = get(flag).downcast::<NSNumber>().unwrap();
            assert!(n.as_bool(), "{flag:?} deveria ser verdadeiro");
        }
        let taps = get(kAudioAggregateDeviceTapListKey)
            .downcast::<NSArray>()
            .unwrap();
        assert_eq!(taps.count(), 1);
        let sub = taps.objectAtIndex(0).downcast::<NSDictionary>().unwrap();
        let sub_uid = sub
            .objectForKey(&key(kAudioSubTapUIDKey))
            .unwrap()
            .downcast::<NSString>()
            .unwrap();
        assert_eq!(sub_uid.to_string(), "TAP-UID");
    }
}
