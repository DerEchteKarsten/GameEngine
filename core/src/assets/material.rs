use bincode::{de::read::Reader, enc::write::Writer};


#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Material {
    pub metalic_factor: f16,
    pub roughness_factor: f16,
    pub color: [f16; 3],
    pub texture_offset: u16,
}

impl bincode::Encode for Material {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> std::result::Result<(), bincode::error::EncodeError> {
        encoder.writer().write(&self.metalic_factor.to_be_bytes())?;
        encoder
            .writer()
            .write(&self.roughness_factor.to_be_bytes())?;
        for i in 0..3 {
            encoder.writer().write(&self.color[i].to_be_bytes())?;
        }
        bincode::Encode::encode(&self.texture_offset, encoder)?;
        std::result::Result::Ok(())
    }
}

impl<Context> bincode::Decode<Context> for Material {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let mut metalic_factor_buf = [0u8; 2];
        decoder.reader().read(&mut metalic_factor_buf)?;
        let mut roughness_buf = [0u8; 2];
        decoder.reader().read(&mut roughness_buf)?;
        let mut color = [0f16; 3];
        let mut color_buf = [0u8; 2];
        for i in 0..3 {
            decoder.reader().read(&mut color_buf)?;
            color[i] = f16::from_be_bytes(color_buf);
        }
        let texture_offset = bincode::Decode::decode(decoder)?;
        std::result::Result::Ok(Self {
            metalic_factor: f16::from_be_bytes(metalic_factor_buf),
            roughness_factor: f16::from_be_bytes(metalic_factor_buf),
            color,
            texture_offset,
        })
    }
}

impl<'a, Context> bincode::BorrowDecode<'a, Context> for Material {
    fn borrow_decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let mut metalic_factor_buf = [0u8; 2];
        decoder.reader().read(&mut metalic_factor_buf)?;
        let mut roughness_buf = [0u8; 2];
        decoder.reader().read(&mut roughness_buf)?;
        let mut color = [0f16; 3];
        let mut color_buf = [0u8; 2];
        for i in 0..3 {
            decoder.reader().read(&mut color_buf)?;
            color[i] = f16::from_be_bytes(color_buf);
        }
        let texture_offset = bincode::Decode::decode(decoder)?;
        std::result::Result::Ok(Self {
            metalic_factor: f16::from_be_bytes(metalic_factor_buf),
            roughness_factor: f16::from_be_bytes(metalic_factor_buf),
            color,
            texture_offset,
        })
    }
}