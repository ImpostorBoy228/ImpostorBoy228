package main

/*
#cgo LDFLAGS: -lm
#include <stdlib.h>
#include "common.h"
*/
import "C"
import (
	"unsafe"
)

func loadFinals(path string) {
	cp := C.CString(path)
	C.load_finals(cp)
	C.free(unsafe.Pointer(cp))
}

func buildSpline() {
	C.build_spline()
}

func interpolateDUT1Spline(mjd float64) float64 {
	return float64(C.interpolate_dut1_spline(C.double(mjd)))
}

func mjdFromUnix(t int64) float64 {
	return float64(C.mjd_from_unix(C.time_t(t)))
}

func computeEarliestNight() (int, int, int, float64) {
	var ey, em, ed C.int
	twi := C.compute_earliest_night(&ey, &em, &ed)
	return int(ey), int(em), int(ed), float64(twi)
}

func getOffset() float64 {
	return float64(C.get_offset())
}

func jdn(y, m, d int) float64 {
	return float64(C.jdn(C.int(y), C.int(m), C.int(d)))
}

func freeEOP() {
	C.free_eop()
}
