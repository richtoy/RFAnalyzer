#
/*
 *    Copyright (C) 2008 .. 2017
 *    Jan van Katwijk (J.vanKatwijk@gmail.com)
 *    Lazy Chair Computing
 *
 *    This file is part of the dab library
 *
 *    dab library is free software; you can redistribute it and/or modify
 *    it under the terms of the GNU General Public License as published by
 *    the Free Software Foundation; either version 2 of the License, or
 *    (at your option) any later version.
 *
 *    dab library is distributed in the hope that it will be useful,
 *    but WITHOUT ANY WARRANTY; without even the implied warranty of
 *    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *    GNU General Public License for more details.
 *
 *    You should have received a copy of the GNU General Public License
 *    along with dab library; if not, write to the Free Software
 *    Foundation, Inc., 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA
 */
#include	"fft-handler.h"
#include	<cstring>

//	Personal RF Analyzer fork: KISS FFT (BSD-3) instead of FFTW3, so the
//	library builds with the Android NDK without extra dependencies.
	fft_handler::fft_handler (uint8_t dabMode): p (dabMode) {
	this	-> fftSize	= p. get_T_u ();
	vector	= new complex<float> [fftSize];
	outVector	= new complex<float> [fftSize];
	for (int i = 0; i < fftSize; i ++)
	   vector [i] = std::complex<float> (0, 0);
	plan	= kiss_fft_alloc (fftSize, 0, nullptr, nullptr);
}

	fft_handler::~fft_handler (void) {
	   kiss_fft_free (plan);
	   delete [] vector;
	   delete [] outVector;
}

void	fft_handler::fft	(Complex *v) {
	kiss_fft (plan, reinterpret_cast<kiss_fft_cpx *>(v),
	                reinterpret_cast<kiss_fft_cpx *>(outVector));
	for (int i = 0; i < fftSize; i ++)
	   v [i] = outVector [i];
}

complex<float>	*fft_handler::getVector () {
	return vector;
}
//
void	fft_handler::do_FFT () {
	kiss_fft (plan, reinterpret_cast<kiss_fft_cpx *>(vector),
	                reinterpret_cast<kiss_fft_cpx *>(outVector));
	for (int i = 0; i < fftSize; i ++)
	   vector [i] = outVector [i];
}

//	Note that we do not scale in case of backwards fft,
//	not needed for our applications
void	fft_handler::do_iFFT (void) {
int	i;
	for (i = 0; i < fftSize; i ++)
	   vector [i] = conj (vector [i]);
	do_FFT ();
	for (i = 0; i < fftSize; i ++)
	   vector [i] = conj (vector [i]);
}
