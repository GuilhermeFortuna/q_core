#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qproperty(QString, version)]
        #[qproperty(QString, contracts_rev)]
        type CoreInfo = super::CoreInfoRust;
    }
}

use cxx_qt_lib::QString;

pub struct CoreInfoRust {
    version: QString,
    contracts_rev: QString,
}

impl Default for CoreInfoRust {
    fn default() -> Self {
        Self {
            version: QString::from(env!("CARGO_PKG_VERSION")),
            contracts_rev: QString::from(include_str!("../../../CONTRACTS_REV").trim()),
        }
    }
}
