use std::time::Duration;

#[cfg(target_os = "macos")]
use mach2::{
    kern_return::{KERN_SUCCESS, kern_return_t},
    mach_time::{mach_timebase_info, mach_timebase_info_data_t},
    thread_policy::{
        THREAD_TIME_CONSTRAINT_POLICY, THREAD_TIME_CONSTRAINT_POLICY_COUNT, thread_policy_set,
        thread_time_constraint_policy,
    },
};

/// Passe le thread courant en politique Mach à contrainte de temps.
#[cfg(target_os = "macos")]
pub fn promote_current_thread(period: Duration, computation: Duration) -> Result<(), String> {
    let timebase = timebase()?;
    let period = absolute_ticks(period, timebase)?;
    let mut policy = thread_time_constraint_policy {
        period,
        computation: absolute_ticks(computation, timebase)?,
        constraint: period,
        preemptible: 1,
    };

    // SAFETY: The pthread port is valid for this thread's lifetime and does not
    // transfer ownership. The policy and count match the Mach ABI for this flavor.
    let code = unsafe {
        thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            THREAD_TIME_CONSTRAINT_POLICY,
            std::ptr::from_mut(&mut policy).cast(),
            THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    check_status(code)
}

#[cfg(not(target_os = "macos"))]
pub fn promote_current_thread(_period: Duration, _computation: Duration) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_status(code: kern_return_t) -> Result<(), String> {
    if code == KERN_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "Priorité temps réel refusée par macOS (code {code})."
        ))
    }
}

#[cfg(target_os = "macos")]
fn timebase() -> Result<mach_timebase_info_data_t, String> {
    let mut info = mach_timebase_info_data_t::default();
    // SAFETY: `info` is writable storage for the Mach timebase structure.
    check_status(unsafe { mach_timebase_info(&mut info) })?;
    Ok(info)
}

#[cfg(target_os = "macos")]
fn absolute_ticks(duration: Duration, timebase: mach_timebase_info_data_t) -> Result<u32, String> {
    duration
        .as_nanos()
        .checked_mul(u128::from(timebase.denom))
        .and_then(|scaled| scaled.checked_div(u128::from(timebase.numer)))
        .and_then(|ticks| u32::try_from(ticks).ok())
        .filter(|&ticks| ticks > 0)
        .ok_or_else(|| "Durée hors limites pour la priorité temps réel de macOS.".into())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use mach2::thread_policy::thread_policy_get;

    #[test]
    fn promotes_current_thread_to_time_constraint_policy() {
        std::thread::spawn(|| {
            let period = Duration::from_millis(5);
            let computation = Duration::from_micros(1_500);
            promote_current_thread(period, computation).unwrap();

            let mut policy = thread_time_constraint_policy {
                period: 0,
                computation: 0,
                constraint: 0,
                preemptible: 0,
            };
            let mut count = THREAD_TIME_CONSTRAINT_POLICY_COUNT;
            let mut get_default = 0;
            // SAFETY: The current pthread owns the port. All output pointers
            // point to writable fields, and `count` describes the policy buffer.
            let code = unsafe {
                thread_policy_get(
                    libc::pthread_mach_thread_np(libc::pthread_self()),
                    THREAD_TIME_CONSTRAINT_POLICY,
                    std::ptr::from_mut(&mut policy).cast(),
                    &mut count,
                    &mut get_default,
                )
            };

            assert_eq!(code, KERN_SUCCESS);
            assert_eq!(count, THREAD_TIME_CONSTRAINT_POLICY_COUNT);
            assert_eq!(get_default, 0);
            let timebase = timebase().unwrap();
            assert_eq!(policy.period, absolute_ticks(period, timebase).unwrap());
            // XNU can raise the computation budget when it is below half the constraint.
            let requested_computation = absolute_ticks(computation, timebase).unwrap();
            assert!((requested_computation..=policy.period).contains(&policy.computation));
            assert_eq!(policy.constraint, policy.period);
            assert_eq!(policy.preemptible, 1);
        })
        .join()
        .unwrap();
    }
}
