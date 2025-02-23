# from\_xyzw

>  Creates a new rotation quaternion.
>  This should generally not be called manually unless you know what you are doing.
>  Use one of the other constructors instead such as `identity` or `from_axis_angle`.
>  `from_xyzw` is mostly used by unit tests and `serde` deserialization.
>  # Preconditions
>  This function does not check if the input is normalized, it is up to the user to
>  provide normalized input or to normalized the resulting quaternion.

#### Arguments

- **x** : `f32` \- No Documentation 🚧
- **y** : `f32` \- No Documentation 🚧
- **z** : `f32` \- No Documentation 🚧
- **w** : `f32` \- No Documentation 🚧

#### Returns

- **arg0** : `Quat` \- No Documentation 🚧