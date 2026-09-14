//! Python projection of columnar bar frames and rolling windows.

use numpy::{PyArray1, PyReadonlyArray1};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyList;
use q_buffers::{
    BarColumns, BarFrame, Bitmap, Column, FrameError, RollingBarWindow, TimeLabel, VolumeSet,
};

fn map_frame_err(err: FrameError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

fn dtype_name(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(obj.getattr("dtype")?.str()?.to_string_lossy().into_owned())
}

fn parse_time_array(time: &Bound<'_, PyAny>) -> PyResult<Vec<i64>> {
    let dtype_str = dtype_name(time)?;
    if dtype_str.contains("datetime64[us]") {
        let view = time.call_method1("view", ("int64",))?;
        let vals: Vec<i64> = view.extract()?;
        if vals.contains(&i64::MIN) {
            return Err(PyValueError::new_err(
                "column `time` contains NaT; missing times are rejected",
            ));
        }
        return Ok(vals);
    }
    if dtype_str.contains("datetime64[ns]") {
        let view = time.call_method1("view", ("int64",))?;
        let vals: Vec<i64> = view.extract()?;
        if vals.contains(&i64::MIN) {
            return Err(PyValueError::new_err(
                "column `time` contains NaT; missing times are rejected",
            ));
        }
        let mut out = Vec::with_capacity(vals.len());
        for (i, &ns) in vals.iter().enumerate() {
            if ns % 1000 != 0 {
                return Err(PyValueError::new_err(format!(
                    "column `time` value at index {i} has a sub-microsecond remainder"
                )));
            }
            out.push(ns / 1000);
        }
        return Ok(out);
    }
    Err(PyTypeError::new_err(format!(
        "column `time` has unsupported dtype {dtype_str}; expected datetime64[us] or datetime64[ns]"
    )))
}

fn require_float64(name: &str, obj: &Bound<'_, PyAny>) -> PyResult<()> {
    let ds = dtype_name(obj)?;
    if ds != "float64" {
        return Err(PyTypeError::new_err(format!(
            "column `{name}` has unsupported dtype {ds}; expected float64"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn bars_from_numpy(
    time: &Bound<'_, PyAny>,
    open: PyReadonlyArray1<'_, f64>,
    high: PyReadonlyArray1<'_, f64>,
    low: PyReadonlyArray1<'_, f64>,
    close: PyReadonlyArray1<'_, f64>,
    tick_volume: Option<PyReadonlyArray1<'_, i64>>,
    spread: Option<PyReadonlyArray1<'_, i64>>,
    real_volume: Option<PyReadonlyArray1<'_, i64>>,
    tz: &str,
) -> PyResult<BarColumns> {
    let label = TimeLabel::parse(tz).map_err(map_frame_err)?;
    let time_vals = parse_time_array(time)?;
    Ok(BarColumns {
        time: time_vals,
        open: open.as_slice()?.to_vec(),
        high: high.as_slice()?.to_vec(),
        low: low.as_slice()?.to_vec(),
        close: close.as_slice()?.to_vec(),
        tick_volume: tick_volume
            .map(|a| a.as_slice().map(|s| s.to_vec()))
            .transpose()?,
        spread: spread
            .map(|a| a.as_slice().map(|s| s.to_vec()))
            .transpose()?,
        real_volume: real_volume
            .map(|a| a.as_slice().map(|s| s.to_vec()))
            .transpose()?,
        label,
    })
}

fn column_to_numpy<'py>(
    py: Python<'py>,
    name: &str,
    frame: &BarFrame,
) -> PyResult<Bound<'py, PyAny>> {
    if name == "time" {
        let np = py.import("numpy")?;
        let arr = np.call_method1("asarray", (frame.time().to_vec(),))?;
        let out = arr.call_method1("astype", ("datetime64[us]",))?;
        return Ok(out);
    }
    match name {
        "open" => return Ok(PyArray1::from_slice(py, frame.open()).into_any()),
        "high" => return Ok(PyArray1::from_slice(py, frame.high()).into_any()),
        "low" => return Ok(PyArray1::from_slice(py, frame.low()).into_any()),
        "close" => return Ok(PyArray1::from_slice(py, frame.close()).into_any()),
        "tick_volume" => {
            let Some(v) = frame.tick_volume() else {
                return Err(PyValueError::new_err("column `tick_volume` is not present"));
            };
            return Ok(PyArray1::from_slice(py, v).into_any());
        }
        "spread" => {
            let Some(v) = frame.spread() else {
                return Err(PyValueError::new_err("column `spread` is not present"));
            };
            return Ok(PyArray1::from_slice(py, v).into_any());
        }
        "real_volume" => {
            let Some(v) = frame.real_volume() else {
                return Err(PyValueError::new_err("column `real_volume` is not present"));
            };
            return Ok(PyArray1::from_slice(py, v).into_any());
        }
        _ => {}
    }
    let Some(col) = frame.column(name) else {
        return Err(PyValueError::new_err(format!("unknown column `{name}`")));
    };
    match col {
        Column::Float64(v) => Ok(PyArray1::from_slice(py, v).into_any()),
        Column::Int64(v) => Ok(PyArray1::from_slice(py, v).into_any()),
        Column::Int8(v) => Ok(PyArray1::from_slice(py, v).into_any()),
        Column::Bool(b) => {
            let bools = b.to_bools();
            Ok(PyArray1::from_slice(py, &bools).into_any())
        }
    }
}

fn parse_derived_column(
    _py: Python<'_>,
    name: &str,
    values: &Bound<'_, PyAny>,
) -> PyResult<Column> {
    let dtype_str = dtype_name(values)?;
    if dtype_str == "float64" {
        let arr: PyReadonlyArray1<'_, f64> = values.extract()?;
        return Ok(Column::Float64(arr.as_slice()?.to_vec()));
    }
    if dtype_str == "int64" {
        let arr: PyReadonlyArray1<'_, i64> = values.extract()?;
        return Ok(Column::Int64(arr.as_slice()?.to_vec()));
    }
    if dtype_str == "int8" {
        let arr: PyReadonlyArray1<'_, i8> = values.extract()?;
        return Ok(Column::Int8(arr.as_slice()?.to_vec()));
    }
    if dtype_str == "bool" || dtype_str == "bool_" {
        let list: Vec<bool> = values.call_method0("tolist")?.extract()?;
        return Ok(Column::Bool(Bitmap::from_bools(&list)));
    }
    Err(PyTypeError::new_err(format!(
        "column `{name}` has unsupported dtype {dtype_str}; expected float64, bool, int8, or int64"
    )))
}

#[pyclass(name = "BarFrame", module = "q_core")]
pub struct PyBarFrame {
    inner: BarFrame,
}

#[pymethods]
impl PyBarFrame {
    #[staticmethod]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (time, open, high, low, close, *, tick_volume=None, spread=None, real_volume=None, tz="naive-wallclock-America/Sao_Paulo"))]
    fn from_numpy(
        py: Python<'_>,
        time: &Bound<'_, PyAny>,
        open: &Bound<'_, PyAny>,
        high: &Bound<'_, PyAny>,
        low: &Bound<'_, PyAny>,
        close: &Bound<'_, PyAny>,
        tick_volume: Option<&Bound<'_, PyAny>>,
        spread: Option<&Bound<'_, PyAny>>,
        real_volume: Option<&Bound<'_, PyAny>>,
        tz: &str,
    ) -> PyResult<Self> {
        let _ = py;
        require_float64("open", open)?;
        require_float64("high", high)?;
        require_float64("low", low)?;
        require_float64("close", close)?;
        let open_a: PyReadonlyArray1<'_, f64> = open.extract()?;
        let high_a: PyReadonlyArray1<'_, f64> = high.extract()?;
        let low_a: PyReadonlyArray1<'_, f64> = low.extract()?;
        let close_a: PyReadonlyArray1<'_, f64> = close.extract()?;
        let tick_a = tick_volume
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let spread_a = spread
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let real_a = real_volume
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let bars = bars_from_numpy(
            time, open_a, high_a, low_a, close_a, tick_a, spread_a, real_a, tz,
        )?;
        Ok(Self {
            inner: BarFrame::try_new(bars).map_err(map_frame_err)?,
        })
    }

    fn add_column(
        &mut self,
        py: Python<'_>,
        name: &str,
        values: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let column = parse_derived_column(py, name, values)?;
        self.inner.push_column(name, column).map_err(map_frame_err)
    }

    fn column<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
        column_to_numpy(py, name, &self.inner)
    }

    fn column_names(&self) -> Vec<String> {
        self.inner
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn schema<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for field in self.inner.schema() {
            let row = (
                field.name.clone(),
                field.arrow_type.to_string(),
                field.nullable,
                field.tz.map(str::to_string),
            );
            list.append(row)?;
        }
        Ok(list)
    }

    #[getter]
    fn tz(&self) -> &'static str {
        self.inner.label().contract_marker()
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }
}

impl PyBarFrame {
    fn from_inner(inner: BarFrame) -> Self {
        Self { inner }
    }
}

#[pyclass(name = "RollingBarWindow", module = "q_core")]
pub struct PyRollingBarWindow {
    inner: RollingBarWindow,
}

#[pymethods]
impl PyRollingBarWindow {
    #[new]
    #[pyo3(signature = (bound, *, tick_volume=false, spread=false, real_volume=false, tz="naive-wallclock-America/Sao_Paulo"))]
    fn new(
        bound: usize,
        tick_volume: bool,
        spread: bool,
        real_volume: bool,
        tz: &str,
    ) -> PyResult<Self> {
        let label = TimeLabel::parse(tz).map_err(map_frame_err)?;
        let volumes = VolumeSet {
            tick_volume,
            spread,
            real_volume,
        };
        Ok(Self {
            inner: RollingBarWindow::new(bound, volumes, label).map_err(map_frame_err)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (time, open, high, low, close, *, tick_volume=None, spread=None, real_volume=None))]
    fn ingest_completed(
        &mut self,
        py: Python<'_>,
        time: &Bound<'_, PyAny>,
        open: &Bound<'_, PyAny>,
        high: &Bound<'_, PyAny>,
        low: &Bound<'_, PyAny>,
        close: &Bound<'_, PyAny>,
        tick_volume: Option<&Bound<'_, PyAny>>,
        spread: Option<&Bound<'_, PyAny>>,
        real_volume: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let _ = py;
        require_float64("open", open)?;
        require_float64("high", high)?;
        require_float64("low", low)?;
        require_float64("close", close)?;
        let open_a: PyReadonlyArray1<'_, f64> = open.extract()?;
        let high_a: PyReadonlyArray1<'_, f64> = high.extract()?;
        let low_a: PyReadonlyArray1<'_, f64> = low.extract()?;
        let close_a: PyReadonlyArray1<'_, f64> = close.extract()?;
        let tick_a = tick_volume
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let spread_a = spread
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let real_a = real_volume
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let bars = bars_from_numpy(
            time,
            open_a,
            high_a,
            low_a,
            close_a,
            tick_a,
            spread_a,
            real_a,
            self.inner.completed().label().contract_marker(),
        )?;
        self.inner.ingest_completed(bars).map_err(map_frame_err)
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (time, open, high, low, close, *, tick_volume=None, spread=None, real_volume=None))]
    fn set_forming(
        &mut self,
        py: Python<'_>,
        time: &Bound<'_, PyAny>,
        open: &Bound<'_, PyAny>,
        high: &Bound<'_, PyAny>,
        low: &Bound<'_, PyAny>,
        close: &Bound<'_, PyAny>,
        tick_volume: Option<&Bound<'_, PyAny>>,
        spread: Option<&Bound<'_, PyAny>>,
        real_volume: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let _ = py;
        require_float64("open", open)?;
        require_float64("high", high)?;
        require_float64("low", low)?;
        require_float64("close", close)?;
        let open_a: PyReadonlyArray1<'_, f64> = open.extract()?;
        let high_a: PyReadonlyArray1<'_, f64> = high.extract()?;
        let low_a: PyReadonlyArray1<'_, f64> = low.extract()?;
        let close_a: PyReadonlyArray1<'_, f64> = close.extract()?;
        let tick_a = tick_volume
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let spread_a = spread
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let real_a = real_volume
            .map(|a| a.extract::<PyReadonlyArray1<'_, i64>>())
            .transpose()?;
        let bars = bars_from_numpy(
            time,
            open_a,
            high_a,
            low_a,
            close_a,
            tick_a,
            spread_a,
            real_a,
            self.inner.completed().label().contract_marker(),
        )?;
        self.inner.set_forming(bars).map_err(map_frame_err)
    }

    fn clear_forming(&mut self) {
        self.inner.clear_forming();
    }

    fn completed(&self) -> PyBarFrame {
        PyBarFrame::from_inner(self.inner.completed().clone())
    }

    fn forming(&self) -> Option<PyBarFrame> {
        self.inner
            .forming()
            .map(|f| PyBarFrame::from_inner(f.clone()))
    }

    #[getter]
    fn bound(&self) -> usize {
        self.inner.bound()
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBarFrame>()?;
    m.add_class::<PyRollingBarWindow>()?;
    Ok(())
}
