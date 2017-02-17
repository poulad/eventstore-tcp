use std::io::{self, Read, Write};
use uuid::Uuid;
use byteorder::{ReadBytesExt, WriteBytesExt, LittleEndian};
use tokio_core::io::{Codec, EasyBuf};

use errors::ErrorKind;
use package::{self, Package, TcpFlags};
use {Message, UsernamePassword};

pub struct PackageCodec;

impl PackageCodec {
    fn decode_inner(&mut self, buf: &mut EasyBuf) -> io::Result<Option<Package>> {
        if buf.len() < 4 + 1 + 1 + 16 {
            return Ok(None);
        }

        let len = {
            let mut cursor = io::Cursor::new(buf.as_slice());
            cursor.read_u32::<LittleEndian>()?
        } as usize;

        if len < 18 {
            panic!("length is too little: {}", len);
        }

        if buf.len() < len + 4 {
            return Ok(None);
        }

        let mut frame = buf.clone();
        frame.drain_to(4);
        frame.split_off(len);

        let decoded_frame = self.decode_body(&mut frame);

        match decoded_frame {
            Ok((c, a, m)) => {
                buf.drain_to(4 + len);
                Ok(Some(Package {
                    correlation_id: c,
                    authentication: a,
                    message: m,
                }))
            }
            Err(e) => Err(e),
        }
    }

    fn decode_body(&mut self,
                   buf: &mut EasyBuf)
                   -> io::Result<(Uuid, Option<UsernamePassword>, Message)> {
        let (d, c, a) = self.decode_header(buf)?;
        let message = Message::decode(d, buf)?;
        Ok((c, a, message))
    }

    fn decode_header(&mut self,
                     buf: &mut EasyBuf)
                     -> io::Result<(u8, Uuid, Option<UsernamePassword>)> {
        let (d, c, a, pos) = {
            let mut cursor = io::Cursor::new(buf.as_slice());
            let discriminator = cursor.read_u8()?;
            let flags = cursor.read_u8()?;
            let flags = match TcpFlags::from_bits(flags) {
                Some(flags) => flags,
                None => bail!(ErrorKind::InvalidFlags(flags)),
            };

            let correlation_id = {
                let mut uuid_bytes = [0u8; 16];
                cursor.read_exact(&mut uuid_bytes)?;
                // this should only err if len is not 16
                Uuid::from_bytes(&uuid_bytes).unwrap()
            };

            let authentication = if flags.contains(package::FLAG_AUTHENTICATED) {
                Some(UsernamePassword::decode(&mut cursor)?)
            } else {
                None
            };

            (discriminator, correlation_id, authentication, cursor.position() as usize)
        };

        buf.drain_to(pos);
        Ok((d, c, a))
    }
}

impl Codec for PackageCodec {
    type In = Package;
    type Out = Package;

    fn decode(&mut self, buf: &mut EasyBuf) -> io::Result<Option<Self::In>> {
        self.decode_inner(buf).map_err(|e| e.into())
    }

    fn encode(&mut self, msg: Package, buf: &mut Vec<u8>) -> io::Result<()> {
        // not sure how to make this without tmp vec
        let mut cursor = io::Cursor::new(Vec::new());

        let mut flags = package::FLAG_NONE;
        if msg.authentication.is_some() {
            flags.insert(package::FLAG_AUTHENTICATED);
        }

        cursor.write_u32::<LittleEndian>(0)?; // placeholder for prefix
        cursor.write_u8(msg.message.discriminator())?;
        cursor.write_u8(flags.bits())?;
        cursor.write_all(msg.correlation_id.as_bytes())?;
        if flags.contains(package::FLAG_AUTHENTICATED) {
            msg.authentication
                .expect("According to flag authentication token is present")
                .encode(&mut cursor)?;
        }

        msg.message.encode(&mut cursor)?;

        let at_end = cursor.position();
        let len = at_end as u32 - 4;

        cursor.set_position(0);
        cursor.write_u32::<LittleEndian>(len)?;

        let tmp = cursor.into_inner();
        buf.extend(tmp);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use rustc_serialize::hex::FromHex;
    use tokio_core::io::Codec;
    use uuid::Uuid;
    use super::{PackageCodec};
    use package::{self, Package};
    use errors;
    use {Message, WriteEventsCompleted};

    #[test]
    fn decode_ping() {
        test_decoding_hex("1200000003007b50a1b034b9224e8f9d708c394fab2d",
                          PackageCodec,
                          Package {
                              authentication: None,
                              correlation_id:
                                  Uuid::parse_str("7b50a1b0-34b9-224e-8f9d-708c394fab2d").unwrap(),
                              message: Message::Ping,
                          });
    }

    #[test]
    fn decode_ping_with_junk() {
        test_decoding_hex("1300000003007b50a1b034b9224e8f9d708c394fab2d00",
                          PackageCodec,
                          Package {
                              authentication: None,
                              correlation_id:
                                  Uuid::parse_str("7b50a1b0-34b9-224e-8f9d-708c394fab2d").unwrap(),
                              message: Message::Ping,
                          });
    }

    #[test]
    fn encode_ping() {
        test_encoding_hex("1200000003007b50a1b034b9224e8f9d708c394fab2d",
                          PackageCodec,
                          Package {
                              authentication: None,
                              correlation_id:
                                  Uuid::parse_str("7b50a1b0-34b9-224e-8f9d-708c394fab2d").unwrap(),
                              message: Message::Ping,
                          });
    }

    #[test]
    fn decode_unknown_discriminator() {
        use std::io;

        let err = PackageCodec.decode(&mut ("12000000ff007b50a1b034b9224e8f9d708c394fab2d"
                .to_string()
                .from_hex()
                .unwrap()
                .into()))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Other);
        let err = err.into_inner();
        match err {
            Some(inner) => {
                match *inner.downcast::<errors::Error>().unwrap() {
                    errors::Error(errors::ErrorKind::UnsupportedDiscriminator(0xff), _) => { /* good */ }
                    x => panic!("unexpected errorkind: {:?}", x),
                }
            }
            x => panic!("unexpected inner error: {:?}", x),
        }
    }

    #[test]
    fn decode_write_events_completed() {
        let input = "2200000083009b59d8734e9fd84eb8a421f2666a3aa40800181e20272884d6bc563084d6bc56";
        test_decoding_hex(input,
                          PackageCodec,
                          Package {
                              authentication: None,
                              correlation_id:
                                  Uuid::parse_str("9b59d873-4e9f-d84e-b8a4-21f2666a3aa4").unwrap(),
                              message: Message::WriteEventsCompleted(Ok(WriteEventsCompleted {
                                  event_numbers: 30..40,
                                  prepare_position: Some(181349124),
                                  commit_position: Some(181349124)
                              }))
                          });
    }

    /*#[test]
    fn decode_wec2() {
        use protobuf;
        use std::io;;
        use pb_client_messages::WriteEventsCompleted as PBWEC;
        let input = "0800181e20272884d6bc563084d6bc56".to_string().from_hex().unwrap();
        println!("{:?}", input);
        let mut cursor = io::Cursor::new(input);
        let parsed = protobuf::parse_from_reader::<PBWEC>(&mut cursor).unwrap();
        println!("{:?}", parsed);
    }*/

    #[test]
    fn encode_write_events_completed() {
        test_encoding_hex("2200000083009b59d8734e9fd84eb8a421f2666a3aa40800181e20272884d6bc563084d6bc56",
                          PackageCodec,
                          Package {
                              authentication: None,
                              correlation_id:
                                  Uuid::parse_str("9b59d873-4e9f-d84e-b8a4-21f2666a3aa4").unwrap(),
                              message: Message::WriteEventsCompleted(Ok(WriteEventsCompleted {
                                  event_numbers: 30..40,
                                  prepare_position: Some(181349124),
                                  commit_position: Some(181349124)
                              }))
                          });

    }

    fn test_decoding_hex<C: Codec>(input: &str, codec: C, expected: C::In)
        where C::In: Debug + PartialEq
    {
        test_decoding(input.to_string().from_hex().unwrap(), codec, expected);
    }

    fn test_decoding<C: Codec>(input: Vec<u8>, mut codec: C, expected: C::In)
        where C::In: Debug + PartialEq
    {
        // decode whole buffer
        {
            let mut buf = input.clone().into();
            let item = codec.decode(&mut buf).unwrap().unwrap();

            assert_eq!(item, expected);
            assert_eq!(buf.len(), 0, "decoding correctly sized buffer left bytes");
        }

        // decoding partial buffer consumes no bytes
        for len in 1..(input.len() - 1) {
            let mut part = input.clone();
            part.truncate(len);

            let mut buf = part.into();
            assert!(codec.decode(&mut buf).unwrap().is_none());
            assert_eq!(buf.len(), len, "decoding partial buffer consumed bytes");
        }

        // decoding a too long buffer consumes no extra bytes
        {
            let mut input = input.clone();
            let len = input.len();
            input.extend(vec![0u8; len]);
            let mut buf = input.into();
            let item = codec.decode(&mut buf).unwrap().unwrap();
            assert_eq!(item, expected);
            assert_eq!(buf.len(), len, "decoding oversized buffer overused bytes");
        }
    }

    fn test_encoding_hex<C: Codec>(input: &str, codec: C, expected: C::Out)
        where C::In: Debug + PartialEq
    {
        test_encoding(input.to_string().from_hex().unwrap(), codec, expected);
    }

    fn test_encoding<C: Codec>(input: Vec<u8>, mut codec: C, expected: C::Out)
        where C::In: Debug + PartialEq
    {
        let mut buf = Vec::new();
        codec.encode(expected, &mut buf).unwrap();
        assert_eq!(buf.as_slice(),
                   input.as_slice(),
                   "encoding did not yield same");
    }
}
