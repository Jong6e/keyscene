//! The standard Windows "open" dialog for choosing a program, file or folder.

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_NODEREFERENCELINKS, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

pub enum Kind {
    /// .exe or .lnk (shortcuts are kept as shortcuts, not followed)
    Program,
    File,
    Folder,
}

/// Blocks until the user picks or cancels. Runs its own STA thread for the dialog.
pub fn pick(kind: Kind, owner: Option<isize>) -> Option<String> {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| -> windows::core::Result<String> {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM;
            match kind {
                Kind::Folder => options |= FOS_PICKFOLDERS,
                Kind::Program => {
                    options |= FOS_NODEREFERENCELINKS;
                    dialog.SetFileTypes(&[
                        COMDLG_FILTERSPEC { pszName: w!("프로그램"), pszSpec: w!("*.exe;*.lnk") },
                        COMDLG_FILTERSPEC { pszName: w!("모든 파일"), pszSpec: w!("*.*") },
                    ])?;
                }
                Kind::File => {}
            }
            dialog.SetOptions(options)?;
            dialog.Show(owner.map(|h| HWND(h as _)))?;
            let item = dialog.GetResult()?;
            let p = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as _));
            Ok(path)
        })();
        CoUninitialize();
        result.ok().filter(|p| !p.is_empty())
    })
    .join()
    .ok()
    .flatten()
}
