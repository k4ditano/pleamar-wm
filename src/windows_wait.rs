//! Native event wakeups and a precise deadline without changing the system's
//! timer period. An idle preview waits for capture, window or renderer events.
use std::{sync::Arc,time::Duration};
use windows::{core::Result,Win32::{Foundation::*,System::Threading::*,UI::WindowsAndMessaging::*}};

pub(super) struct Wake(HANDLE);
// Kernel events are signaled across threads; Arc keeps the handle alive.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}
impl Wake {
    pub fn new() -> Result<Arc<Self>> { Ok(Arc::new(Self(unsafe { CreateEventW(None,false,false,None) }?))) }
    pub fn signal(&self) { let _=unsafe { SetEvent(self.0) }; }
}
impl Drop for Wake { fn drop(&mut self) { let _=unsafe { CloseHandle(self.0) }; } }

pub(super) struct Waiter { timer:HANDLE,wake:Arc<Wake> }
impl Waiter {
    pub fn new(wake:Arc<Wake>) -> Result<Self> {
        // WGC's supported Windows versions also provide high-resolution timers.
        let timer=unsafe { CreateWaitableTimerExW(None,None,CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,TIMER_ALL_ACCESS.0) }?;
        Ok(Self {timer,wake})
    }
    pub fn wait(&self,duration:Duration) -> Result<()> {
        let due=-(duration.as_nanos().div_ceil(100).clamp(1,i64::MAX as u128) as i64);
        unsafe {
            SetWaitableTimer(self.timer,&due,0,None,None,false)?;
            let result=MsgWaitForMultipleObjectsEx(Some(&[self.wake.0,self.timer]),INFINITE,QS_ALLINPUT,MWMO_INPUTAVAILABLE);
            if result==WAIT_FAILED { return Err(windows::core::Error::from_thread()); }
        }
        Ok(())
    }
}
impl Drop for Waiter { fn drop(&mut self) { let _=unsafe { CloseHandle(self.timer) }; } }
